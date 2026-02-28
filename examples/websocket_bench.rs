//! WebSocket performance benchmark comparing tokio vs io_uring
//!
//! Run tokio: cargo run --example websocket_bench --release
//! Run io_uring: cargo run --example websocket_bench --release --features io-uring

use fastwebsockets::{Frame, OpCode, Role, WebSocket, Payload};
use std::time::Instant;

async fn websocket_benchmark(backend: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== {} WebSocket Benchmark ===", backend);
    
    #[cfg(feature = "io-uring")]
    {
        use fastwebsockets::uring;
        
        let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
        let addr = listener.local_addr()?;
        
        // Server task
        let server = tokio_uring::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = WebSocket::after_handshake(stream, Role::Server);
            ws.set_auto_close(true);
            
            for _ in 0..100 {
                let frame = ws.read_frame().await.unwrap();
                if frame.opcode == OpCode::Text {
                    ws.write_frame(frame).await.unwrap();
                } else if frame.opcode == OpCode::Close {
                    break;
                }
            }
        });
        
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        
        let start = Instant::now();
        
        // Client
        let stream = uring::TcpStream::connect(addr).await?;
        let mut ws = WebSocket::after_handshake(stream, Role::Client);
        
        for i in 0..100 {
            let msg = format!("Message {}", i);
            ws.write_frame(Frame::text(Payload::Owned(msg.into_bytes()))).await?;
            let _response = ws.read_frame().await?;
        }
        
        ws.write_frame(Frame::close(1000, b"")).await?;
        
        let elapsed = start.elapsed();
        println!("✅ 100 WebSocket echo messages in {:?}", elapsed);
        println!("📊 {:.2} ms per message", elapsed.as_millis() as f64 / 100.0);
        
        server.abort();
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        
        // Server
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = WebSocket::after_handshake(stream, Role::Server);
            ws.set_auto_close(true);
            
            for _ in 0..100 {
                let frame = ws.read_frame().await.unwrap();
                if frame.opcode == OpCode::Text {
                    ws.write_frame(frame).await.unwrap();
                } else if frame.opcode == OpCode::Close {
                    break;
                }
            }
        });
        
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        
        let start = Instant::now();
        
        let stream = tokio::net::TcpStream::connect(addr).await?;
        let mut ws = WebSocket::after_handshake(stream, Role::Client);
        
        for i in 0..100 {
            let msg = format!("Message {}", i);
            ws.write_frame(Frame::text(Payload::Owned(msg.into_bytes()))).await?;
            let _response = ws.read_frame().await?;
        }
        
        ws.write_frame(Frame::close(1000, b"")).await?;
        
        let elapsed = start.elapsed();
        println!("✅ 100 WebSocket echo messages in {:?}", elapsed);
        println!("📊 {:.2} ms per message", elapsed.as_millis() as f64 / 100.0);
        
        server.abort();
    }
    
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            websocket_benchmark("io_uring").await
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        tokio::runtime::Runtime::new()?.block_on(async {
            websocket_benchmark("tokio").await
        })
    }
}