//! Performance comparison between tokio and io_uring backends
//!
//! Run tokio version: cargo run --example performance_test --release
//! Run io_uring version: cargo run --example performance_test --release --features io-uring

use fastwebsockets::{Frame, OpCode, Role, WebSocket, Payload};
use std::time::Instant;
use std::sync::Arc;
use tokio::sync::Barrier;

async fn run_echo_test(
    runtime_name: &str,
    num_clients: usize,
    messages_per_client: usize,
) -> Result<std::time::Duration, Box<dyn std::error::Error>> {
    println!("🚀 Starting {} echo test: {} clients, {} messages each", 
             runtime_name, num_clients, messages_per_client);
    
    let barrier = Arc::new(Barrier::new(num_clients + 1));
    let start_time = Arc::new(std::sync::Mutex::new(None::<Instant>));
    
    #[cfg(feature = "io-uring")]
    {
        use fastwebsockets::uring;
        
        let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
        let addr = listener.local_addr()?;
        
        // Server
        let server_barrier = barrier.clone();
        let server_start_time = start_time.clone();
        let server_handle = tokio_uring::spawn(async move {
            server_barrier.wait().await;
            let start = Instant::now();
            *server_start_time.lock().unwrap() = Some(start);
            
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Server);
                ws.set_auto_close(true);
                ws.set_auto_pong(true);
                
                tokio_uring::spawn(async move {
                    let mut msg_count = 0;
                    loop {
                        let frame = ws.read_frame().await.unwrap();
                        if frame.opcode == OpCode::Close {
                            break;
                        } else if frame.opcode == OpCode::Text {
                            ws.write_frame(frame).await.unwrap();
                            msg_count += 1;
                            if msg_count >= messages_per_client {
                                break;
                            }
                        }
                    }
                });
            }
        });
        
        // Clients
        let mut client_handles = Vec::new();
        for client_id in 0..num_clients {
            let client_barrier = barrier.clone();
            let handle = tokio_uring::spawn(async move {
                client_barrier.wait().await;
                
                let stream = uring::TcpStream::connect(addr).await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Client);
                
                for msg_id in 0..messages_per_client {
                    let message = format!("Hello from client {} message {}", client_id, msg_id);
                    ws.write_frame(Frame::text(Payload::Owned(message.into_bytes()))).await.unwrap();
                    let _response = ws.read_frame().await.unwrap();
                }
                
                ws.write_frame(Frame::close(1000, b"")).await.unwrap();
            });
            client_handles.push(handle);
        }
        
        // Start all tasks
        barrier.wait().await;
        
        // Wait for clients
        for handle in client_handles {
            handle.await.unwrap();
        }
        
        server_handle.abort();
        
        let start = start_time.lock().unwrap().unwrap();
        Ok(start.elapsed())
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        
        // Server
        let server_barrier = barrier.clone();
        let server_start_time = start_time.clone();
        let server_handle = tokio::spawn(async move {
            server_barrier.wait().await;
            let start = Instant::now();
            *server_start_time.lock().unwrap() = Some(start);
            
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Server);
                ws.set_auto_close(true);
                ws.set_auto_pong(true);
                
                tokio::spawn(async move {
                    let mut msg_count = 0;
                    loop {
                        let frame = ws.read_frame().await.unwrap();
                        if frame.opcode == OpCode::Close {
                            break;
                        } else if frame.opcode == OpCode::Text {
                            ws.write_frame(frame).await.unwrap();
                            msg_count += 1;
                            if msg_count >= messages_per_client {
                                break;
                            }
                        }
                    }
                });
            }
        });
        
        // Clients
        let mut client_handles = Vec::new();
        for client_id in 0..num_clients {
            let client_barrier = barrier.clone();
            let handle = tokio::spawn(async move {
                client_barrier.wait().await;
                
                let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Client);
                
                for msg_id in 0..messages_per_client {
                    let message = format!("Hello from client {} message {}", client_id, msg_id);
                    ws.write_frame(Frame::text(Payload::Owned(message.into_bytes()))).await.unwrap();
                    let _response = ws.read_frame().await.unwrap();
                }
                
                ws.write_frame(Frame::close(1000, b"")).await.unwrap();
            });
            client_handles.push(handle);
        }
        
        // Start all tasks
        barrier.wait().await;
        
        // Wait for clients
        for handle in client_handles {
            handle.await?;
        }
        
        server_handle.abort();
        
        let start = start_time.lock().unwrap().unwrap();
        Ok(start.elapsed())
    }
}

async fn run_benchmark_suite() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    let backend = "io_uring";
    #[cfg(not(feature = "io-uring"))]
    let backend = "tokio";
    
    println!("=== fastwebsockets Echo Server Performance Test ===");
    println!("Backend: {}", backend);
    println!("");
    
    // Test scenarios
    let scenarios = [
        (1, 100),   // Single client, many messages
        (5, 50),    // Few clients, moderate messages  
        (10, 20),   // Many clients, few messages
    ];
    
    for (clients, messages) in scenarios.iter() {
        let elapsed = run_echo_test(backend, *clients, *messages).await?;
        let total_messages = clients * messages;
        let messages_per_sec = total_messages as f64 / elapsed.as_secs_f64();
        
        println!("✅ {} clients × {} msgs = {} total messages in {:?}", 
                 clients, messages, total_messages, elapsed);
        println!("   📊 {:.0} messages/second", messages_per_sec);
        println!("   📊 {:.2} ms per message", elapsed.as_millis() as f64 / total_messages as f64);
        println!("");
    }
    
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            run_benchmark_suite().await
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run_benchmark_suite())
    }
}