//! Benchmark comparison between tokio and io_uring backends
//!
//! Run tokio version: cargo run --example bench_comparison
//! Run io_uring version: cargo run --example bench_comparison --features io-uring

use fastwebsockets::{Frame, OpCode, Role, WebSocket, Payload};
use std::time::{Duration, Instant};

async fn run_echo_test(runtime_name: &str, num_messages: usize) -> Result<Duration, Box<dyn std::error::Error>> {
    println!("Running {} echo test with {} messages...", runtime_name, num_messages);
    
    let start = Instant::now();
    
    #[cfg(feature = "io-uring")]
    {
        use fastwebsockets::uring;
        
        let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
        let addr = listener.local_addr()?;
        
        // Server task
        let server = tokio_uring::spawn(async move {
            for _ in 0..2 { // Accept 2 connections for this test
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Server);
                ws.set_auto_close(true);
                ws.set_auto_pong(true);
                
                // Handle multiple messages
                for _ in 0..num_messages {
                    let frame = ws.read_frame().await.unwrap();
                    if frame.opcode == OpCode::Text {
                        ws.write_frame(frame).await.unwrap(); // Echo back
                    } else if frame.opcode == OpCode::Close {
                        break;
                    }
                }
            }
        });
        
        // Client tasks
        let client1 = tokio_uring::spawn(async move {
            let stream = uring::TcpStream::connect(addr).await.unwrap();
            let mut ws = WebSocket::after_handshake(stream, Role::Client);
            
            for i in 0..num_messages {
                let msg = format!("Message {}", i);
                ws.write_frame(Frame::text(Payload::Owned(msg.into_bytes()))).await.unwrap();
                let _response = ws.read_frame().await.unwrap();
            }
            
            ws.write_frame(Frame::close(1000, b"")).await.unwrap();
        });
        
        let client2 = tokio_uring::spawn(async move {
            let stream = uring::TcpStream::connect(addr).await.unwrap();
            let mut ws = WebSocket::after_handshake(stream, Role::Client);
            
            for i in 0..num_messages {
                let msg = format!("Message {}", i);
                ws.write_frame(Frame::text(Payload::Owned(msg.into_bytes()))).await.unwrap();
                let _response = ws.read_frame().await.unwrap();
            }
            
            ws.write_frame(Frame::close(1000, b"")).await.unwrap();
        });
        
        // Wait for completion
        let _ = tokio::try_join!(server, client1, client2);
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        
        // Server task
        let server = tokio::spawn(async move {
            for _ in 0..2 { // Accept 2 connections for this test
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = WebSocket::after_handshake(stream, Role::Server);
                ws.set_auto_close(true);
                ws.set_auto_pong(true);
                
                // Handle multiple messages
                for _ in 0..num_messages {
                    let frame = ws.read_frame().await.unwrap();
                    if frame.opcode == OpCode::Text {
                        ws.write_frame(frame).await.unwrap(); // Echo back
                    } else if frame.opcode == OpCode::Close {
                        break;
                    }
                }
            }
        });
        
        // Client tasks
        let client1 = tokio::spawn(async move {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let mut ws = WebSocket::after_handshake(stream, Role::Client);
            
            for i in 0..num_messages {
                let msg = format!("Message {}", i);
                ws.write_frame(Frame::text(Payload::Owned(msg.into_bytes()))).await.unwrap();
                let _response = ws.read_frame().await.unwrap();
            }
            
            ws.write_frame(Frame::close(1000, b"")).await.unwrap();
        });
        
        let client2 = tokio::spawn(async move {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let mut ws = WebSocket::after_handshake(stream, Role::Client);
            
            for i in 0..num_messages {
                let msg = format!("Message {}", i);
                ws.write_frame(Frame::text(Payload::Owned(msg.into_bytes()))).await.unwrap();
                let _response = ws.read_frame().await.unwrap();
            }
            
            ws.write_frame(Frame::close(1000, b"")).await.unwrap();
        });
        
        // Wait for completion  
        let _ = tokio::try_join!(server, client1, client2);
    }
    
    Ok(start.elapsed())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            println!("=== io_uring Backend Performance Test ===");
            
            let elapsed_100 = run_echo_test("io_uring", 100).await?;
            println!("io_uring 100 messages: {:?}", elapsed_100);
            
            let elapsed_1000 = run_echo_test("io_uring", 1000).await?;
            println!("io_uring 1000 messages: {:?}", elapsed_1000);
            
            Ok::<(), Box<dyn std::error::Error>>(())
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        
        rt.block_on(async {
            println!("=== Tokio Backend Performance Test ===");
            
            let elapsed_100 = run_echo_test("tokio", 100).await?;
            println!("tokio 100 messages: {:?}", elapsed_100);
            
            let elapsed_1000 = run_echo_test("tokio", 1000).await?;
            println!("tokio 1000 messages: {:?}", elapsed_1000);
            
            Ok::<(), Box<dyn std::error::Error>>(())
        })
    }
}