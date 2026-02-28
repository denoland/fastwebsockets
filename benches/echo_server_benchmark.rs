use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use fastwebsockets::{Frame, OpCode, Role, WebSocket, Payload};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Barrier;

async fn run_echo_benchmark(
    runtime_name: &str,
    num_clients: usize,
    messages_per_client: usize,
    message_size: usize,
) -> Result<Duration, Box<dyn std::error::Error + Send + Sync>> {
    let barrier = Arc::new(Barrier::new(num_clients + 1)); // +1 for server
    let start_time = std::sync::Arc::new(std::sync::Mutex::new(None));
    
    #[cfg(feature = "io-uring")]
    {
        use fastwebsockets::uring;
        
        let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
        let addr = listener.local_addr()?;
        
        // Server task
        let server_barrier = barrier.clone();
        let server_start_time = start_time.clone();
        let server_handle = tokio_uring::spawn(async move {
            server_barrier.wait().await;
            let start = std::time::Instant::now();
            *server_start_time.lock().unwrap() = Some(start);
            
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Server);
                ws.set_auto_close(true);
                ws.set_auto_pong(true);
                
                tokio_uring::spawn(async move {
                    for _ in 0..messages_per_client {
                        let frame = ws.read_frame().await.unwrap();
                        if frame.opcode == OpCode::Text || frame.opcode == OpCode::Binary {
                            ws.write_frame(frame).await.unwrap();
                        } else if frame.opcode == OpCode::Close {
                            break;
                        }
                    }
                });
            }
        });
        
        // Client tasks
        let mut client_handles = Vec::new();
        for _ in 0..num_clients {
            let client_barrier = barrier.clone();
            let message = "x".repeat(message_size);
            let handle = tokio_uring::spawn(async move {
                client_barrier.wait().await;
                
                let stream = uring::TcpStream::connect(addr).await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Client);
                
                for _ in 0..messages_per_client {
                    ws.write_frame(Frame::text(Payload::Owned(message.clone().into_bytes()))).await.unwrap();
                    let _response = ws.read_frame().await.unwrap();
                }
                
                ws.write_frame(Frame::close(1000, b"")).await.unwrap();
            });
            client_handles.push(handle);
        }
        
        // Start benchmark
        barrier.wait().await;
        
        // Wait for all clients to finish
        for handle in client_handles {
            handle.await.unwrap();
        }
        
        // Stop server
        drop(server_handle);
        
        let start = start_time.lock().unwrap().unwrap();
        Ok(start.elapsed())
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        
        // Server task
        let server_barrier = barrier.clone();
        let server_start_time = start_time.clone();
        let server_handle = tokio::spawn(async move {
            server_barrier.wait().await;
            let start = std::time::Instant::now();
            *server_start_time.lock().unwrap() = Some(start);
            
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Server);
                ws.set_auto_close(true);
                ws.set_auto_pong(true);
                
                tokio::spawn(async move {
                    for _ in 0..messages_per_client {
                        let frame = ws.read_frame().await.unwrap();
                        if frame.opcode == OpCode::Text || frame.opcode == OpCode::Binary {
                            ws.write_frame(frame).await.unwrap();
                        } else if frame.opcode == OpCode::Close {
                            break;
                        }
                    }
                });
            }
        });
        
        // Client tasks
        let mut client_handles = Vec::new();
        for _ in 0..num_clients {
            let client_barrier = barrier.clone();
            let message = "x".repeat(message_size);
            let handle = tokio::spawn(async move {
                client_barrier.wait().await;
                
                let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Client);
                
                for _ in 0..messages_per_client {
                    ws.write_frame(Frame::text(Payload::Owned(message.clone().into_bytes()))).await.unwrap();
                    let _response = ws.read_frame().await.unwrap();
                }
                
                ws.write_frame(Frame::close(1000, b"")).await.unwrap();
            });
            client_handles.push(handle);
        }
        
        // Start benchmark
        barrier.wait().await;
        
        // Wait for all clients to finish
        for handle in client_handles {
            handle.await?;
        }
        
        // Stop server
        server_handle.abort();
        
        let start = start_time.lock().unwrap().unwrap();
        Ok(start.elapsed())
    }
}

fn bench_echo_server_small_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("echo_server_small");
    
    for &num_clients in [1, 5, 10].iter() {
        let messages_per_client = 100;
        let message_size = 64; // 64 bytes
        
        group.throughput(Throughput::Elements((num_clients * messages_per_client) as u64));
        
        #[cfg(not(feature = "io-uring"))]
        group.bench_with_input(
            BenchmarkId::new("tokio", format!("{}clients_{}msgs", num_clients, messages_per_client)),
            &(num_clients, messages_per_client, message_size),
            |b, &(clients, msgs, size)| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                b.to_async(&rt).iter(|| async {
                    black_box(
                        run_echo_benchmark("tokio", clients, msgs, size)
                            .await
                            .unwrap()
                    )
                })
            },
        );
        
        #[cfg(feature = "io-uring")]
        group.bench_with_input(
            BenchmarkId::new("io_uring", format!("{}clients_{}msgs", num_clients, messages_per_client)),
            &(num_clients, messages_per_client, message_size),
            |b, &(clients, msgs, size)| {
                b.iter(|| {
                    black_box(fastwebsockets::uring::start(async {
                        run_echo_benchmark("io_uring", clients, msgs, size)
                            .await
                            .unwrap()
                    }))
                })
            },
        );
    }
    
    group.finish();
}

fn bench_echo_server_large_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("echo_server_large");
    group.sample_size(10); // Fewer samples for large message tests
    
    for &message_size in [1024, 4096, 16384].iter() {
        let num_clients = 5;
        let messages_per_client = 50;
        
        group.throughput(Throughput::Bytes((num_clients * messages_per_client * message_size) as u64));
        
        #[cfg(not(feature = "io-uring"))]
        group.bench_with_input(
            BenchmarkId::new("tokio", format!("{}bytes", message_size)),
            &(num_clients, messages_per_client, message_size),
            |b, &(clients, msgs, size)| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                b.to_async(&rt).iter(|| async {
                    black_box(
                        run_echo_benchmark("tokio", clients, msgs, size)
                            .await
                            .unwrap()
                    )
                })
            },
        );
        
        #[cfg(feature = "io-uring")]
        group.bench_with_input(
            BenchmarkId::new("io_uring", format!("{}bytes", message_size)),
            &(num_clients, messages_per_client, message_size),
            |b, &(clients, msgs, size)| {
                b.iter(|| {
                    black_box(fastwebsockets::uring::start(async {
                        run_echo_benchmark("io_uring", clients, msgs, size)
                            .await
                            .unwrap()
                    }))
                })
            },
        );
    }
    
    group.finish();
}

fn bench_echo_server_high_concurrency(c: &mut Criterion) {
    let mut group = c.benchmark_group("echo_server_concurrency");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30)); // Longer measurement for stability
    
    for &num_clients in [20, 50].iter() {
        let messages_per_client = 20;
        let message_size = 256;
        
        group.throughput(Throughput::Elements((num_clients * messages_per_client) as u64));
        
        #[cfg(not(feature = "io-uring"))]
        group.bench_with_input(
            BenchmarkId::new("tokio", format!("{}clients", num_clients)),
            &(num_clients, messages_per_client, message_size),
            |b, &(clients, msgs, size)| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                b.to_async(&rt).iter(|| async {
                    black_box(
                        run_echo_benchmark("tokio", clients, msgs, size)
                            .await
                            .unwrap()
                    )
                })
            },
        );
        
        #[cfg(feature = "io-uring")]
        group.bench_with_input(
            BenchmarkId::new("io_uring", format!("{}clients", num_clients)),
            &(num_clients, messages_per_client, message_size),
            |b, &(clients, msgs, size)| {
                b.iter(|| {
                    black_box(fastwebsockets::uring::start(async {
                        run_echo_benchmark("io_uring", clients, msgs, size)
                            .await
                            .unwrap()
                    }))
                })
            },
        );
    }
    
    group.finish();
}

#[cfg(feature = "io-uring")]
criterion_group!(
    benches,
    bench_echo_server_small_messages,
    bench_echo_server_large_messages,
    bench_echo_server_high_concurrency
);

#[cfg(not(feature = "io-uring"))]
criterion_group!(
    benches,
    bench_echo_server_small_messages,
    bench_echo_server_large_messages,
    bench_echo_server_high_concurrency
);

criterion_main!(benches);