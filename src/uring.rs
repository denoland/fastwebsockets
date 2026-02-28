//! io_uring integration for fastwebsockets
//!
//! This module provides io_uring-backed networking types when the `io-uring` feature is enabled.

#[cfg(feature = "io-uring")]
pub mod net {
    use std::io;
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};
    use std::sync::{Arc, Mutex};
    use std::collections::VecDeque;
    
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    /// Pending read operation
    struct PendingRead {
        waker: Waker,
        buf_size: usize,
        result: Option<io::Result<Vec<u8>>>,
    }

    /// Pending write operation  
    struct PendingWrite {
        waker: Waker,
        data: Vec<u8>,
        result: Option<io::Result<usize>>,
    }

    /// State for managing async operations
    struct StreamState {
        pending_reads: VecDeque<PendingRead>,
        pending_writes: VecDeque<PendingWrite>,
    }

    /// Wrapper around tokio_uring::net::TcpStream that implements AsyncRead/AsyncWrite
    /// 
    /// Uses a task-based adapter to bridge io_uring's ownership model with AsyncRead/AsyncWrite.
    pub struct TcpStream {
        inner: Arc<tokio::sync::Mutex<tokio_uring::net::TcpStream>>,
        state: Arc<Mutex<StreamState>>,
    }

    impl TcpStream {
        pub async fn connect(addr: SocketAddr) -> io::Result<Self> {
            let inner = tokio_uring::net::TcpStream::connect(addr).await?;
            Ok(Self { 
                inner: Arc::new(tokio::sync::Mutex::new(inner)),
                state: Arc::new(Mutex::new(StreamState {
                    pending_reads: VecDeque::new(),
                    pending_writes: VecDeque::new(),
                })),
            })
        }

        pub fn from_std(socket: std::net::TcpStream) -> Self {
            let inner = tokio_uring::net::TcpStream::from_std(socket);
            Self { 
                inner: Arc::new(tokio::sync::Mutex::new(inner)),
                state: Arc::new(Mutex::new(StreamState {
                    pending_reads: VecDeque::new(),
                    pending_writes: VecDeque::new(),
                })),
            }
        }
        
        /// Convert back to the underlying io_uring stream
        /// Note: This consumes the wrapper and extracts the inner stream
        pub fn into_inner(self) -> Result<tokio_uring::net::TcpStream, &'static str> {
            Arc::try_unwrap(self.inner)
                .map_err(|_| "Stream still in use")
                .map(|mutex| mutex.into_inner())
        }
    }

    impl AsyncRead for TcpStream {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let remaining = buf.remaining();
            if remaining == 0 {
                return Poll::Ready(Ok(()));
            }

            let mut state = self.state.lock().unwrap();
            
            // Check if we have a completed read
            if let Some(mut pending) = state.pending_reads.pop_front() {
                if let Some(result) = pending.result.take() {
                    match result {
                        Ok(data) => {
                            let to_copy = data.len().min(remaining);
                            buf.put_slice(&data[..to_copy]);
                            return Poll::Ready(Ok(()));
                        }
                        Err(e) => return Poll::Ready(Err(e)),
                    }
                } else {
                    // Still pending, put it back and return pending
                    pending.waker = cx.waker().clone();
                    state.pending_reads.push_front(pending);
                    return Poll::Pending;
                }
            }

            // Start a new read operation
            let stream = self.inner.clone();
            let state_clone = self.state.clone();
            let waker = cx.waker().clone();
            
            let pending = PendingRead {
                waker: waker.clone(),
                buf_size: remaining,
                result: None,
            };
            state.pending_reads.push_back(pending);
            drop(state);

            // Spawn the read operation
            tokio_uring::spawn(async move {
                let mut guard = stream.lock().await;
                let read_buf = vec![0u8; remaining];
                let (result, returned_buf) = guard.read(read_buf).await;
                
                let final_result = match result {
                    Ok(n) => {
                        if n == 0 {
                            Ok(vec![])  // EOF
                        } else {
                            Ok(returned_buf[..n].to_vec())
                        }
                    }
                    Err(e) => Err(e),
                };

                // Store result and wake
                let mut state = state_clone.lock().unwrap();
                if let Some(pending) = state.pending_reads.back_mut() {
                    pending.result = Some(final_result);
                    pending.waker.wake_by_ref();
                }
            });

            Poll::Pending
        }
    }

    impl AsyncWrite for TcpStream {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, io::Error>> {
            if buf.is_empty() {
                return Poll::Ready(Ok(0));
            }

            let mut state = self.state.lock().unwrap();
            
            // Check if we have a completed write
            if let Some(mut pending) = state.pending_writes.pop_front() {
                if let Some(result) = pending.result.take() {
                    return Poll::Ready(result);
                } else {
                    // Still pending, put it back
                    pending.waker = cx.waker().clone();
                    state.pending_writes.push_front(pending);
                    return Poll::Pending;
                }
            }

            // Start a new write operation
            let stream = self.inner.clone();
            let state_clone = self.state.clone();
            let waker = cx.waker().clone();
            let write_data = buf.to_vec();
            let write_len = write_data.len();
            
            let pending = PendingWrite {
                waker: waker.clone(),
                data: write_data.clone(),
                result: None,
            };
            state.pending_writes.push_back(pending);
            drop(state);

            // Spawn the write operation
            tokio_uring::spawn(async move {
                let mut guard = stream.lock().await;
                let (result, _) = guard.write(write_data).submit().await;
                
                let final_result = match result {
                    Ok(_) => Ok(write_len),
                    Err(e) => Err(e),
                };

                // Store result and wake
                let mut state = state_clone.lock().unwrap();
                if let Some(pending) = state.pending_writes.back_mut() {
                    pending.result = Some(final_result);
                    pending.waker.wake_by_ref();
                }
            });

            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    /// Wrapper around tokio_uring::net::TcpListener  
    pub struct TcpListener {
        inner: tokio_uring::net::TcpListener,
    }

    impl TcpListener {
        pub fn bind(addr: SocketAddr) -> io::Result<Self> {
            let inner = tokio_uring::net::TcpListener::bind(addr)?;
            Ok(Self { inner })
        }

        pub fn from_std(listener: std::net::TcpListener) -> Self {
            let inner = tokio_uring::net::TcpListener::from_std(listener);
            Self { inner }
        }

        pub fn local_addr(&self) -> io::Result<SocketAddr> {
            self.inner.local_addr()
        }

        pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
            let (stream, addr) = self.inner.accept().await?;
            let wrapped_stream = TcpStream { 
                inner: Arc::new(tokio::sync::Mutex::new(stream)),
                state: Arc::new(Mutex::new(StreamState {
                    pending_reads: VecDeque::new(),
                    pending_writes: VecDeque::new(),
                })),
            };
            Ok((wrapped_stream, addr))
        }
    }
}

#[cfg(feature = "io-uring")]
pub use tokio_uring::start;

#[cfg(not(feature = "io-uring"))]
pub mod net {
    pub use tokio::net::{TcpListener, TcpStream};
}

#[cfg(not(feature = "io-uring"))]
pub fn start<F, R>(future: F) -> R
where
    F: std::future::Future<Output = R>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(future)
}

/// Convenience re-export for conditional networking types
pub use net::*;