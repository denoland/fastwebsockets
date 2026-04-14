use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fastwebsockets::{Frame, OpCode, Role, WebSocket};
use std::time::Duration;

#[cfg(feature = "io-uring")]
mod uring_bench {
    use super::*;
    use fastwebsockets::uring;
    
    pub async fn echo_server_uring() -> Result<(), Box<dyn std::error::Error>> {
        let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap()).await?;
        let addr = listener.local_addr()?;
        
        // Spawn server
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut ws = WebSocket::after_handshake(stream, Role::Server);
                    ws.set_auto_close(true);
                    ws.set_auto_pong(true);
                    
                    while let Ok(frame) = ws.read_frame().await {
                        if frame.opcode == OpCode::Close {
                            break;
                        }
                        if matches!(frame.opcode, OpCode::Text | OpCode::Binary) {
                            let _ = ws.write_frame(frame).await;
                        }
                    }
                });
            }
        });
        
        // Client connection
        let stream = uring::TcpStream::connect(addr).await?;
        let mut ws = WebSocket::after_handshake(stream, Role::Client);
        
        let message = "Hello, world!";
        for _ in 0..1000 {
            ws.write_frame(Frame::text(message.as_bytes().to_vec())).await?;
            let _response = ws.read_frame().await?;
        }
        
        ws.write_frame(Frame::close(1000, b"")).await?;
        Ok(())
    }
}

mod tokio_bench {
    use super::*;
    
    pub async fn echo_server_tokio() -> Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        
        // Spawn server
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut ws = WebSocket::after_handshake(stream, Role::Server);
                    ws.set_auto_close(true);
                    ws.set_auto_pong(true);
                    
                    while let Ok(frame) = ws.read_frame().await {
                        if frame.opcode == OpCode::Close {
                            break;
                        }
                        if matches!(frame.opcode, OpCode::Text | OpCode::Binary) {
                            let _ = ws.write_frame(frame).await;
                        }
                    }
                });
            }
        });
        
        // Client connection
        let stream = tokio::net::TcpStream::connect(addr).await?;
        let mut ws = WebSocket::after_handshake(stream, Role::Client);
        
        let message = "Hello, world!";
        for _ in 0..1000 {
            ws.write_frame(Frame::text(message.as_bytes().to_vec())).await?;
            let _response = ws.read_frame().await?;
        }
        
        ws.write_frame(Frame::close(1000, b"")).await?;
        Ok(())
    }
}

fn bench_tokio_echo(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    
    c.bench_function("tokio_echo_1k_messages", |b| {
        b.to_async(&rt).iter(|| async {
            black_box(tokio_bench::echo_server_tokio().await.unwrap())
        })
    });
}

#[cfg(feature = "io-uring")]
fn bench_uring_echo(c: &mut Criterion) {
    c.bench_function("uring_echo_1k_messages", |b| {
        b.iter(|| {
            black_box(fastwebsockets::uring::start(async {
                uring_bench::echo_server_uring().await.unwrap()
            }))
        })
    });
}

#[cfg(feature = "io-uring")]
criterion_group!(benches, bench_tokio_echo, bench_uring_echo);

#[cfg(not(feature = "io-uring"))]
criterion_group!(benches, bench_tokio_echo);

criterion_main!(benches);