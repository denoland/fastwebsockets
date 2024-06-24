// Copyright 2023 Divy Srivastava <dj.srivastava23@gmail.com>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! _fastwebsockets_ is a minimal, fast WebSocket server implementation.
//!
//! [https://github.com/denoland/fastwebsockets](https://github.com/denoland/fastwebsockets)
//!
//! Passes the _Autobahn|TestSuite_ and fuzzed with LLVM's _libfuzzer_.
//!
//! You can use it as a raw websocket frame parser and deal with spec compliance yourself, or you can use it as a full-fledged websocket server.
//!
//! # Example
//!
//! ```
//! use tokio::net::TcpStream;
//! use fastwebsockets::{WebSocket, OpCode, Role};
//! use anyhow::Result;
//!
//! async fn handle(
//!   socket: TcpStream,
//! ) -> Result<()> {
//!   let mut ws = WebSocket::after_handshake(socket, Role::Server);
//!   ws.set_writev(false);
//!   ws.set_auto_close(true);
//!   ws.set_auto_pong(true);
//!
//!   loop {
//!     let frame = ws.read_frame().await?;
//!     match frame.opcode {
//!       OpCode::Close => break,
//!       OpCode::Text | OpCode::Binary => {
//!         ws.write_frame(frame).await?;
//!       }
//!       _ => {}
//!     }
//!   }
//!   Ok(())
//! }
//! ```
//!
//! ## Fragmentation
//!
//! By default, fastwebsockets will give the application raw frames with FIN set. Other
//! crates like tungstenite which will give you a single message with all the frames
//! concatenated.
//!
//! For concanated frames, use `FragmentCollector`:
//! ```
//! use fastwebsockets::{FragmentCollector, WebSocket, Role};
//! use tokio::net::TcpStream;
//! use anyhow::Result;
//!
//! async fn handle(
//!   socket: TcpStream,
//! ) -> Result<()> {
//!   let mut ws = WebSocket::after_handshake(socket, Role::Server);
//!   let mut ws = FragmentCollector::new(ws);
//!   let incoming = ws.read_frame().await?;
//!   // Always returns full messages
//!   assert!(incoming.fin);
//!   Ok(())
//! }
//! ```
//!
//! _permessage-deflate is not supported yet._
//!
//! ## HTTP Upgrades
//!
//! Enable the `upgrade` feature to do server-side upgrades and client-side
//! handshakes.
//!
//! This feature is powered by [hyper](https://docs.rs/hyper).
//!
//! ```
//! use fastwebsockets::upgrade::upgrade;
//! use http_body_util::Empty;
//! use hyper::{Request, body::{Incoming, Bytes}, Response};
//! use anyhow::Result;
//!
//! async fn server_upgrade(
//!   mut req: Request<Incoming>,
//! ) -> Result<Response<Empty<Bytes>>> {
//!   let (response, fut) = upgrade(&mut req)?;
//!
//!   tokio::spawn(async move {
//!     let ws = fut.await;
//!     // Do something with the websocket
//!   });
//!
//!   Ok(response)
//! }
//! ```
//!
//! Use the `handshake` module for client-side handshakes.
//!
//! ```
//! use fastwebsockets::handshake;
//! use fastwebsockets::FragmentCollector;
//! use hyper::{Request, body::Bytes, upgrade::Upgraded, header::{UPGRADE, CONNECTION}};
//! use http_body_util::Empty;
//! use hyper_util::rt::TokioIo;
//! use tokio::net::TcpStream;
//! use std::future::Future;
//! use anyhow::Result;
//!
//! async fn connect() -> Result<FragmentCollector<TokioIo<Upgraded>>> {
//!   let stream = TcpStream::connect("localhost:9001").await?;
//!
//!   let req = Request::builder()
//!     .method("GET")
//!     .uri("http://localhost:9001/")
//!     .header("Host", "localhost:9001")
//!     .header(UPGRADE, "websocket")
//!     .header(CONNECTION, "upgrade")
//!     .header(
//!       "Sec-WebSocket-Key",
//!       fastwebsockets::handshake::generate_key(),
//!     )
//!     .header("Sec-WebSocket-Version", "13")
//!     .body(Empty::<Bytes>::new())?;
//!
//!   let (ws, _) = handshake::client(&SpawnExecutor, req, stream).await?;
//!   Ok(FragmentCollector::new(ws))
//! }
//!
//! // Tie hyper's executor to tokio runtime
//! struct SpawnExecutor;
//!
//! impl<Fut> hyper::rt::Executor<Fut> for SpawnExecutor
//! where
//!   Fut: Future + Send + 'static,
//!   Fut::Output: Send + 'static,
//! {
//!   fn execute(&self, fut: Fut) {
//!     tokio::task::spawn(fut);
//!   }
//! }
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

mod close;
mod error;
mod fragment;
mod frame;
/// Client handshake.
#[cfg(feature = "upgrade")]
#[cfg_attr(docsrs, doc(cfg(feature = "upgrade")))]
pub mod handshake;
mod mask;
/// HTTP upgrades.
#[cfg(feature = "upgrade")]
#[cfg_attr(docsrs, doc(cfg(feature = "upgrade")))]
pub mod upgrade;

use bytes::Buf;

use bytes::BytesMut;
use futures::task::AtomicWaker;
use std::future::poll_fn;
use std::pin::pin;
use std::sync::Arc;
use std::task::ready;
use std::task::Context;
use std::task::Poll;

#[cfg(feature = "unstable-split")]
use std::future::Future;

use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;

pub use crate::close::CloseCode;
pub use crate::error::WebSocketError;
pub use crate::fragment::FragmentCollector;
#[cfg(feature = "unstable-split")]
pub use crate::fragment::FragmentCollectorRead;
pub use crate::frame::Frame;
pub use crate::frame::OpCode;
pub use crate::frame::Payload;
pub use crate::mask::unmask;

enum ContextKind {
  /// Read is used when the cx is called from WebSocketRead.
  Read,
  /// Write is used when the cx is called from WebSocketWrite.
  Write,
}

// WakerDemux keeps track of whether the waker was called from a reader or a writer.
//
// This is important because the reader can also write, in order to reply to Ping or Close messages.
// If we didn't implement the WakerDemux the reader could hijack the writer's Waker and the writer's task
// would never get notified.
//
// Waking up the WakerDemux will wake the read and write tasks.
#[derive(Default)]
struct WakerDemux {
  read_waker: AtomicWaker,
  write_waker: AtomicWaker,
}

impl futures::task::ArcWake for WakerDemux {
  fn wake_by_ref(this: &Arc<Self>) {
    this.read_waker.wake();
    this.write_waker.wake();
  }
}

impl WakerDemux {
  /// Set the Waker to the corresponding slot.
  #[inline]
  fn set_waker(&self, kind: ContextKind, waker: &futures::task::Waker) {
    match kind {
      ContextKind::Read => {
        self.read_waker.register(waker);
      }
      ContextKind::Write => {
        self.write_waker.register(waker);
      }
    }
  }

  #[inline]
  fn with_context<F, R>(self: &Arc<Self>, f: F) -> R
  where
    F: FnOnce(&mut Context<'_>) -> R,
  {
    let waker = futures::task::waker_ref(&self);
    let mut cx = Context::from_waker(&waker);
    f(&mut cx)
  }
}

/// The role the connection is taking.
///
/// When a server role is taken the frames will not be masked, unlike
/// the client role, in which frames are masked.
#[derive(Copy, Clone, PartialEq)]
pub enum Role {
  Server,
  Client,
}

pub(crate) struct WriteHalf {
  role: Role,
  closed: bool,
  vectored: bool,
  auto_apply_mask: bool,
  writev_threshold: usize,
  buffer: BytesMut,
}

pub(crate) struct ReadHalf {
  role: Role,
  auto_apply_mask: bool,
  auto_close: bool,
  auto_pong: bool,
  writev_threshold: usize,
  max_message_size: usize,
  buffer: BytesMut,
}

#[cfg(feature = "unstable-split")]
/// Read end of a WebSocket connection.
pub struct WebSocketRead<S> {
  stream: S,
  read_half: ReadHalf,
}

#[cfg(feature = "unstable-split")]
/// Write end of a WebSocket connection.
pub struct WebSocketWrite<S> {
  stream: S,
  write_half: WriteHalf,
}

#[cfg(feature = "unstable-split")]
/// Create a split `WebSocketRead`/`WebSocketWrite` pair from a stream that has already completed the WebSocket handshake.
pub fn after_handshake_split<R, W>(
  read: R,
  write: W,
  role: Role,
) -> (WebSocketRead<R>, WebSocketWrite<W>)
where
  R: AsyncWrite + Unpin,
  W: AsyncWrite + Unpin,
{
  (
    WebSocketRead {
      stream: read,
      read_half: ReadHalf::after_handshake(role),
    },
    WebSocketWrite {
      stream: write,
      write_half: WriteHalf::after_handshake(role),
    },
  )
}

#[cfg(feature = "unstable-split")]
impl<'f, S> WebSocketRead<S> {
  /// Consumes the `WebSocketRead` and returns the underlying stream.
  #[inline]
  pub(crate) fn into_parts_internal(self) -> (S, ReadHalf) {
    (self.stream, self.read_half)
  }

  pub fn set_writev_threshold(&mut self, threshold: usize) {
    self.read_half.writev_threshold = threshold;
  }

  /// Sets whether to automatically close the connection when a close frame is received. When set to `false`, the application will have to manually send close frames.
  ///
  /// Default: `true`
  pub fn set_auto_close(&mut self, auto_close: bool) {
    self.read_half.auto_close = auto_close;
  }

  /// Sets whether to automatically send a pong frame when a ping frame is received.
  ///
  /// Default: `true`
  pub fn set_auto_pong(&mut self, auto_pong: bool) {
    self.read_half.auto_pong = auto_pong;
  }

  /// Sets the maximum message size in bytes. If a message is received that is larger than this, the connection will be closed.
  ///
  /// Default: 64 MiB
  pub fn set_max_message_size(&mut self, max_message_size: usize) {
    self.read_half.max_message_size = max_message_size;
  }

  /// Sets whether to automatically apply the mask to the frame payload.
  ///
  /// Default: `true`
  pub fn set_auto_apply_mask(&mut self, auto_apply_mask: bool) {
    self.read_half.auto_apply_mask = auto_apply_mask;
  }

  /// Reads a frame from the stream.
  pub async fn read_frame<R, E>(
    &mut self,
    send_fn: &mut impl FnMut(Frame<'f>) -> R,
  ) -> Result<Frame, WebSocketError>
  where
    S: AsyncRead + Unpin,
    E: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    R: Future<Output = Result<(), E>>,
  {
    loop {
      let (res, obligated_send) =
        self.read_half.read_frame_inner(&mut self.stream).await;
      if let Some(frame) = obligated_send {
        let res = send_fn(frame).await;
        res.map_err(|e| WebSocketError::SendError(e.into()))?;
      }
      if let Some(frame) = res? {
        break Ok(frame);
      }
    }
  }

  /// Reads a frame from the stream.
  pub fn poll_read_frame<R, E>(
    &mut self,
    cx: &mut Context<'_>,
  ) -> Poll<(Result<Option<Frame>, WebSocketError>, Option<Frame>)>
  where
    S: AsyncRead + Unpin,
  {
    self.read_half.poll_read_frame_inner(&mut self.stream, cx)
  }
}

#[cfg(feature = "unstable-split")]
impl<'f, S> WebSocketWrite<S> {
  /// Sets whether to use vectored writes. This option does not guarantee that vectored writes will be always used.
  ///
  /// Default: `true`
  pub fn set_writev(&mut self, vectored: bool) {
    self.write_half.vectored = vectored;
  }

  pub fn set_writev_threshold(&mut self, threshold: usize) {
    self.write_half.writev_threshold = threshold;
  }

  /// Sets whether to automatically apply the mask to the frame payload.
  ///
  /// Default: `true`
  pub fn set_auto_apply_mask(&mut self, auto_apply_mask: bool) {
    self.write_half.auto_apply_mask = auto_apply_mask;
  }

  /// Returns whether the connection was closed or not.
  pub fn is_closed(&self) -> bool {
    self.write_half.closed
  }

  /// Sends a frame.
  pub async fn write_frame(
    &mut self,
    frame: Frame<'f>,
  ) -> Result<(), WebSocketError>
  where
    S: AsyncWrite + Unpin,
  {
    self.write_half.write_frame(&mut self.stream, frame).await
  }

  /// Serializes the frame into the internal buffer and tries to flush the contents.
  ///
  /// If the function returns Poll::Pending, the user needs to call poll_flush.
  pub fn poll_write_frame(
    &mut self,
    cx: &mut Context<'_>,
    frame: Frame<'f>,
  ) -> Poll<Result<(), WebSocketError>>
  where
    S: AsyncWrite + Unpin,
  {
    self.write_half.start_send_frame(frame)?;
    self.write_half.poll_flush(&mut self.stream, cx)
  }
}

/// WebSocket protocol implementation over an async stream.
pub struct WebSocket<S> {
  stream: S,
  write_half: WriteHalf,
  read_half: ReadHalf,
  waker: Arc<WakerDemux>,
}

impl<'f, S> WebSocket<S> {
  /// Creates a new `WebSocket` from a stream that has already completed the WebSocket handshake.
  ///
  /// Use the `upgrade` feature to handle server upgrades and client handshakes.
  ///
  /// # Example
  ///
  /// ```
  /// use tokio::net::TcpStream;
  /// use fastwebsockets::{WebSocket, OpCode, Role};
  /// use anyhow::Result;
  ///
  /// async fn handle_client(
  ///   socket: TcpStream,
  /// ) -> Result<()> {
  ///   let mut ws = WebSocket::after_handshake(socket, Role::Server);
  ///   // ...
  ///   Ok(())
  /// }
  /// ```
  pub fn after_handshake(stream: S, role: Role) -> Self
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    let waker = Arc::new(WakerDemux::default());
    Self {
      stream,
      waker,
      write_half: WriteHalf::after_handshake(role),
      read_half: ReadHalf::after_handshake(role),
    }
  }

  #[cfg(feature = "unstable-split")]
  /// Split a [`WebSocket`] into a [`WebSocketRead`] and [`WebSocketWrite`] half. Note that the split version does not
  /// handle fragmented packets and you may wish to create a [`FragmentCollectorRead`] over top of the read half that
  /// is returned.
  pub fn split<R, W>(
    self,
    split_fn: impl Fn(S) -> (R, W),
  ) -> (WebSocketRead<R>, WebSocketWrite<W>)
  where
    S: AsyncRead + AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
  {
    let (stream, read, write) = self.into_parts_internal();
    let (r, w) = split_fn(stream);
    (
      WebSocketRead {
        stream: r,
        read_half: read,
      },
      WebSocketWrite {
        stream: w,
        write_half: write,
      },
    )
  }

  /// Consumes the `WebSocket` and returns the underlying stream.
  #[inline]
  pub fn into_inner(self) -> S {
    // self.write_half.into_inner().stream
    self.stream
  }

  /// Consumes the `WebSocket` and returns the underlying stream.
  #[inline]
  pub(crate) fn into_parts_internal(self) -> (S, ReadHalf, WriteHalf) {
    (self.stream, self.read_half, self.write_half)
  }

  /// Sets whether to use vectored writes. This option does not guarantee that vectored writes will be always used.
  ///
  /// Default: `true`
  pub fn set_writev(&mut self, vectored: bool) {
    self.write_half.vectored = vectored;
  }

  pub fn set_writev_threshold(&mut self, threshold: usize) {
    self.read_half.writev_threshold = threshold;
    self.write_half.writev_threshold = threshold;
  }

  /// Sets whether to automatically close the connection when a close frame is received. When set to `false`, the application will have to manually send close frames.
  ///
  /// Default: `true`
  pub fn set_auto_close(&mut self, auto_close: bool) {
    self.read_half.auto_close = auto_close;
  }

  /// Sets whether to automatically send a pong frame when a ping frame is received.
  ///
  /// Default: `true`
  pub fn set_auto_pong(&mut self, auto_pong: bool) {
    self.read_half.auto_pong = auto_pong;
  }

  /// Sets the maximum message size in bytes. If a message is received that is larger than this, the connection will be closed.
  ///
  /// Default: 64 MiB
  pub fn set_max_message_size(&mut self, max_message_size: usize) {
    self.read_half.max_message_size = max_message_size;
  }

  /// Sets whether to automatically apply the mask to the frame payload.
  ///
  /// Default: `true`
  pub fn set_auto_apply_mask(&mut self, auto_apply_mask: bool) {
    self.read_half.auto_apply_mask = auto_apply_mask;
    self.write_half.auto_apply_mask = auto_apply_mask;
  }

  /// Returns whether the connection is closed or not.
  pub fn is_closed(&self) -> bool {
    self.write_half.closed
  }

  /// Writes a frame to the stream.
  ///
  /// # Example
  ///
  /// ```
  /// use fastwebsockets::{WebSocket, Frame, OpCode};
  /// use tokio::net::TcpStream;
  /// use anyhow::Result;
  ///
  /// async fn send(
  ///   ws: &mut WebSocket<TcpStream>
  /// ) -> Result<()> {
  ///   let mut frame = Frame::binary(vec![0x01, 0x02, 0x03].into());
  ///   ws.write_frame(frame).await?;
  ///   Ok(())
  /// }
  /// ```
  pub async fn write_frame(
    &mut self,
    frame: Frame<'f>,
  ) -> Result<(), WebSocketError>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    self.write_half.write_frame(&mut self.stream, frame).await?;
    Ok(())
  }

  /// Serializes a frame into the internal buffer.
  ///
  /// This method is similar to [Sink::start_send](https://docs.rs/futures/0.3.30/futures/sink/trait.Sink.html#tymethod.start_send).
  pub fn start_send_frame(
    &mut self,
    frame: Frame<'f>,
  ) -> Result<(), WebSocketError>
  where
    S: AsyncWrite + Unpin,
  {
    self.write_half.start_send_frame(frame)
  }

  /// Serializes a frame into the internal buffer.
  ///
  /// Beware of the internal buffer. If the other end of the connection is not consuming fast enough it might fill fast.
  ///
  /// This method is similar to [Sink::start_send](https://docs.rs/futures/0.3.30/futures/sink/trait.Sink.html#tymethod.start_send).
  pub fn poll_write_frame(
    &mut self,
    cx: &mut Context<'_>,
    frame: Frame<'f>,
  ) -> Poll<Result<(), WebSocketError>>
  where
    S: AsyncWrite + Unpin,
  {
    self.write_half.start_send_frame(frame)?;
    self.poll_flush(cx)
  }

  /// Flushes the internal buffer into the Stream.
  ///
  /// Returns Poll::Ready(Ok(())) when no more bytes are left.
  pub fn poll_flush(
    &mut self,
    cx: &mut Context<'_>,
  ) -> Poll<Result<(), WebSocketError>>
  where
    S: AsyncWrite + Unpin,
  {
    self.waker.set_waker(ContextKind::Write, cx.waker());
    self
      .waker
      .with_context(|cx| self.write_half.poll_flush(&mut self.stream, cx))
  }

  /// Reads a frame from the stream.
  ///
  /// This method will unmask the frame payload. For fragmented frames, use `FragmentCollector::read_frame`.
  ///
  /// Text frames payload is guaranteed to be valid UTF-8.
  ///
  /// # Example
  ///
  /// ```
  /// use fastwebsockets::{OpCode, WebSocket, Frame};
  /// use tokio::net::TcpStream;
  /// use anyhow::Result;
  ///
  /// async fn echo(
  ///   ws: &mut WebSocket<TcpStream>
  /// ) -> Result<()> {
  ///   let frame = ws.read_frame().await?;
  ///   match frame.opcode {
  ///     OpCode::Text | OpCode::Binary => {
  ///       ws.write_frame(frame).await?;
  ///     }
  ///     _ => {}
  ///   }
  ///   Ok(())
  /// }
  /// ```
  pub async fn read_frame(&mut self) -> Result<Frame<'f>, WebSocketError>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    poll_fn(|cx| self.poll_read_frame(cx)).await
  }

  /// Polls the next frame from the Stream.
  pub fn poll_read_frame(
    &mut self,
    cx: &mut Context<'_>,
  ) -> Poll<Result<Frame<'f>, WebSocketError>>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    loop {
      let (res, obligated_send) =
        ready!(self.read_half.poll_read_frame_inner(&mut self.stream, cx));

      let is_closed = self.write_half.closed;
      if let Some(frame) = obligated_send {
        if !is_closed {
          self.write_half.start_send_frame(frame)?;
          self.waker.set_waker(ContextKind::Read, cx.waker());
          let res = self.waker.with_context(|cx| {
            self.write_half.poll_flush(&mut self.stream, cx)
          });
          ready!(res)?;
        }
      }

      if let Some(frame) = res? {
        if is_closed && frame.opcode != OpCode::Close {
          return Poll::Ready(Err(WebSocketError::ConnectionClosed));
        }

        break Poll::Ready(Ok(frame));
      }
    }
  }
}

impl ReadHalf {
  pub fn after_handshake(role: Role) -> Self {
    let buffer = BytesMut::with_capacity(8192);

    Self {
      role,
      auto_apply_mask: true,
      auto_close: true,
      auto_pong: true,
      writev_threshold: 1024,
      max_message_size: 64 << 20,
      buffer,
    }
  }

  /// Attempt to read a single frame from from the incoming stream, returning any send obligations if
  /// `auto_close` or `auto_pong` are enabled. Callers to this function are obligated to send the
  /// frame in the latter half of the tuple if one is specified, unless the write half of this socket
  /// has been closed.
  ///
  /// XXX: Do not expose this method to the public API.
  pub(crate) async fn read_frame_inner<'f, S>(
    &mut self,
    stream: &mut S,
  ) -> (Result<Option<Frame<'f>>, WebSocketError>, Option<Frame<'f>>)
  where
    S: AsyncRead + Unpin,
  {
    poll_fn(|cx| self.poll_read_frame_inner(stream, cx)).await
  }

  /// Reads a frame from the Stream.
  pub(crate) fn poll_read_frame_inner<'f, S>(
    &mut self,
    stream: &mut S,
    cx: &mut Context<'_>,
  ) -> Poll<(Result<Option<Frame<'f>>, WebSocketError>, Option<Frame<'f>>)>
  where
    S: AsyncRead + Unpin,
  {
    let mut frame = match ready!(self.poll_parse_frame_header(stream, cx)) {
      Ok(frame) => frame,
      Err(e) => return Poll::Ready((Err(e), None)),
    };

    if self.role == Role::Server && self.auto_apply_mask {
      frame.unmask()
    };

    match frame.opcode {
      OpCode::Close if self.auto_close => {
        match frame.payload.len() {
          0 => {}
          1 => {
            return Poll::Ready((Err(WebSocketError::InvalidCloseFrame), None))
          }
          _ => {
            let code = close::CloseCode::from(u16::from_be_bytes(
              frame.payload[0..2].try_into().unwrap(),
            ));

            #[cfg(feature = "simd")]
            if simdutf8::basic::from_utf8(&frame.payload[2..]).is_err() {
              return Poll::Ready((Err(WebSocketError::InvalidUTF8), None));
            };

            #[cfg(not(feature = "simd"))]
            if std::str::from_utf8(&frame.payload[2..]).is_err() {
              return (Err(WebSocketError::InvalidUTF8), None);
            };

            if !code.is_allowed() {
              return Poll::Ready((
                Err(WebSocketError::InvalidCloseCode),
                Some(Frame::close(1002, &frame.payload[2..])),
              ));
            }
          }
        };

        let obligated_send = Frame::close_raw(frame.payload.to_owned().into());
        Poll::Ready((Ok(Some(frame)), Some(obligated_send)))
      }
      OpCode::Ping if self.auto_pong => {
        Poll::Ready((Ok(None), Some(Frame::pong(frame.payload))))
      }
      OpCode::Text => {
        if frame.fin && !frame.is_utf8() {
          Poll::Ready((Err(WebSocketError::InvalidUTF8), None))
        } else {
          Poll::Ready((Ok(Some(frame)), None))
        }
      }
      _ => Poll::Ready((Ok(Some(frame)), None)),
    }
  }

  /// Reads a frame from the Stream parsing the headers.
  fn poll_parse_frame_header<'a, S>(
    &mut self,
    stream: &mut S,
    cx: &mut Context<'_>,
  ) -> Poll<Result<Frame<'a>, WebSocketError>>
  where
    S: AsyncRead + Unpin,
  {
    macro_rules! read_next {
      () => {{
        let bytes_read = ready!(tokio_util::io::poll_read_buf(
          pin!(&mut *stream),
          cx,
          &mut self.buffer
        ))?;
        if bytes_read == 0 {
          return Poll::Ready(Err(WebSocketError::UnexpectedEOF));
        }
      }};
    }

    // Read the first two bytes
    while self.buffer.remaining() < 2 {
      read_next!();
    }

    let fin = self.buffer[0] & 0b10000000 != 0;
    let rsv1 = self.buffer[0] & 0b01000000 != 0;
    let rsv2 = self.buffer[0] & 0b00100000 != 0;
    let rsv3 = self.buffer[0] & 0b00010000 != 0;

    if rsv1 || rsv2 || rsv3 {
      return Poll::Ready(Err(WebSocketError::ReservedBitsNotZero));
    }

    let opcode = frame::OpCode::try_from(self.buffer[0] & 0b00001111)?;
    let masked = self.buffer[1] & 0b10000000 != 0;

    let length_code = self.buffer[1] & 0x7F;
    let extra = match length_code {
      126 => 2,
      127 => 8,
      _ => 0,
    };

    // total header size
    let header_size = 2 + extra + masked as usize * 4;
    while self.buffer.remaining() < header_size {
      read_next!();
    }

    let mut header = &self.buffer[2..header_size];

    let payload_len: usize = match extra {
      0 => usize::from(length_code),
      2 => header.get_u16() as usize,
      #[cfg(any(target_pointer_width = "64", target_pointer_width = "128"))]
      8 => header.get_u64() as usize,
      // On 32bit systems, usize is only 4bytes wide so we must check for usize overflowing
      #[cfg(any(
        target_pointer_width = "8",
        target_pointer_width = "16",
        target_pointer_width = "32"
      ))]
      8 => match usize::try_from(header.get_u64()) {
        Ok(length) => length,
        Err(_) => return Err(WebSocketError::FrameTooLarge),
      },
      _ => unreachable!(),
    };

    let mask = if masked {
      Some(header.get_u32().to_be_bytes())
    } else {
      None
    };

    if frame::is_control(opcode) && !fin {
      return Poll::Ready(Err(WebSocketError::ControlFrameFragmented));
    }

    if opcode == OpCode::Ping && payload_len > 125 {
      return Poll::Ready(Err(WebSocketError::PingFrameTooLarge));
    }

    if payload_len >= self.max_message_size {
      return Poll::Ready(Err(WebSocketError::FrameTooLarge));
    }

    // Reserve a bit more to try to get next frame header and avoid a syscall to read it next time
    while header_size + payload_len > self.buffer.remaining() {
      read_next!();
    }

    // if we read too much it will stay in the buffer, for the next call to this method
    let mut message = self.buffer.split_to(payload_len + header_size);
    // split the message off of header_size to get the payload.
    let payload = message.split_off(header_size);
    let frame = Frame::new(fin, opcode, mask, Payload::Bytes(payload));
    Poll::Ready(Ok(frame))
  }
}

impl WriteHalf {
  pub fn after_handshake(role: Role) -> Self {
    Self {
      role,
      closed: false,
      auto_apply_mask: true,
      vectored: true,
      writev_threshold: 1024,
      buffer: BytesMut::with_capacity(1024),
    }
  }

  /// Writes a frame to the provided stream.
  pub async fn write_frame<'a, S>(
    &'a mut self,
    stream: &mut S,
    frame: Frame<'a>,
  ) -> Result<(), WebSocketError>
  where
    S: AsyncWrite + Unpin,
  {
    // maybe_frame determines the state.
    // If a frame is present we need to poll_ready, else flush it.
    let mut maybe_frame = Some(frame);
    poll_fn(|cx| loop {
      match maybe_frame.take() {
        Some(frame) => match self.poll_ready(stream, cx) {
          Poll::Ready(res) => {
            res?;
            self.start_send_frame(frame)?;
          }
          Poll::Pending => {
            maybe_frame = Some(frame);
            return Poll::Pending;
          }
        },
        None => {
          return self.poll_flush(stream, cx);
        }
      }
    })
    .await
  }

  /// Ensures that the underlying connection is ready. It will try to flush the contents if any.
  ///
  /// If you prefer to buffer requests as much as possible you can skip this step, generally and
  /// call start_send_frame.
  pub fn poll_ready<S>(
    &mut self,
    stream: &mut S,
    cx: &mut Context<'_>,
  ) -> Poll<Result<(), WebSocketError>>
  where
    S: AsyncWrite + Unpin,
  {
    if self.buffer.is_empty() {
      Poll::Ready(Ok(()))
    } else {
      let written = ready!(pin!(&mut *stream).poll_write(cx, &self.buffer))?;
      if written == 0 {
        Poll::Ready(Err(WebSocketError::ConnectionClosed))
      } else {
        Poll::Ready(Ok(()))
      }
    }
  }

  pub fn start_send_frame<'a>(
    &'a mut self,
    mut frame: Frame<'a>,
  ) -> Result<(), WebSocketError> {
    // TODO: backpressure check?

    if self.role == Role::Client && self.auto_apply_mask {
      frame.mask();
    }

    if frame.opcode == OpCode::Close {
      self.closed = true;
    } else if self.closed {
      return Err(WebSocketError::ConnectionClosed);
    }

    // TODO(dgrr): Cap max payload size with a user setting?

    frame.fmt_head(&mut self.buffer);
    self.buffer.extend_from_slice(&frame.payload);

    Ok(())
  }

  pub fn poll_flush<'a, S>(
    &'a mut self,
    stream: &mut S,
    cx: &mut Context<'_>,
  ) -> Poll<Result<(), WebSocketError>>
  where
    S: AsyncWrite + Unpin,
  {
    loop {
      // try to flush before writing
      if let Err(err) =
        ready!(pin!(&mut *stream).poll_flush(cx)).map_err(Into::into)
      {
        break Poll::Ready(Err(err));
      }

      if self.buffer.is_empty() {
        break Poll::Ready(Ok(()));
      }

      let written = ready!(pin!(&mut *stream).poll_write(cx, &self.buffer))?;
      if written == 0 {
        break Poll::Ready(Err(WebSocketError::ConnectionClosed));
      }

      self.buffer.advance(written);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const _: () = {
    const fn assert_unsync<S>() {
      // Generic trait with a blanket impl over `()` for all types.
      trait AmbiguousIfImpl<A> {
        // Required for actually being able to reference the trait.
        fn some_item() {}
      }

      impl<T: ?Sized> AmbiguousIfImpl<()> for T {}

      // Used for the specialized impl when *all* traits in
      // `$($t)+` are implemented.
      #[allow(dead_code)]
      struct Invalid;

      impl<T: ?Sized + Sync> AmbiguousIfImpl<Invalid> for T {}

      // If there is only one specialized trait impl, type inference with
      // `_` can be resolved and this can compile. Fails to compile if
      // `$x` implements `AmbiguousIfImpl<Invalid>`.
      let _ = <S as AmbiguousIfImpl<_>>::some_item;
    }
    assert_unsync::<WebSocket<tokio::net::TcpStream>>();
  };
}
