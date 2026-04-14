//! Steady-state performance comparison (after warmup)
//!
//! Run tokio: cargo run --example steady_state_bench --release
//! Run io_uring: cargo run --example steady_state_bench --release --features io-uring

use std::time::Instant;

async fn steady_state_test(backend: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== {} Steady-State Performance ===", backend);
    
    #[cfg(feature = "io-uring")]
    {
        use fastwebsockets::uring;
        
        let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
        let addr = listener.local_addr()?;
        
        let server = tokio_uring::spawn(async move {
            // Warmup
            for _ in 0..5 {
                let (_stream, _) = listener.accept().await.unwrap();
            }
            
            // Measure steady-state
            let mut connect_times = Vec::new();
            let mut io_times = Vec::new();
            
            for _ in 0..50 {
                let accept_start = Instant::now();
                let (stream, _) = listener.accept().await.unwrap();
                connect_times.push(accept_start.elapsed());
                
                let native_stream = stream.into_inner();
                
                let io_start = Instant::now();
                let buffer = vec![0u8; 64];
                let (result, _) = native_stream.read(buffer).await;
                if let Ok(n) = result {
                    if n > 0 {
                        let response = b"echo";
                        let (result, _) = native_stream.write_all(response.to_vec()).await;
                        if result.is_ok() {
                            io_times.push(io_start.elapsed());
                        }
                    }
                }
            }
            
            let avg_connect = connect_times.iter().sum::<std::time::Duration>() / connect_times.len() as u32;
            let avg_io = io_times.iter().sum::<std::time::Duration>() / io_times.len() as u32;
            
            println!("📊 io_uring steady-state averages:");
            println!("   Connect: {:?}", avg_connect);
            println!("   I/O: {:?}", avg_io);
        });
        
        // Warmup connections
        for _ in 0..5 {
            let _ = uring::TcpStream::connect(addr).await;
        }
        
        // Steady state connections
        for _ in 0..50 {
            let stream = uring::TcpStream::connect(addr).await?;
            let native_stream = stream.into_inner();
            
            let message = b"test";
            let (result, _) = native_stream.write_all(message.to_vec()).await;
            result?;
            
            let buffer = vec![0u8; 64];
            let (result, _) = native_stream.read(buffer).await;
            result?;
        }
        
        server.await.unwrap();
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            
            // Warmup
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 64];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(b"echo").await;
            }
            
            // Measure steady-state
            let mut connect_times = Vec::new();
            let mut io_times = Vec::new();
            
            for _ in 0..50 {
                let accept_start = Instant::now();
                let (mut stream, _) = listener.accept().await.unwrap();
                connect_times.push(accept_start.elapsed());
                
                let io_start = Instant::now();
                let mut buffer = [0u8; 64];
                if let Ok(n) = stream.read(&mut buffer).await {
                    if n > 0 {
                        if stream.write_all(b"echo").await.is_ok() {
                            io_times.push(io_start.elapsed());
                        }
                    }
                }
            }
            
            let avg_connect = connect_times.iter().sum::<std::time::Duration>() / connect_times.len() as u32;
            let avg_io = io_times.iter().sum::<std::time::Duration>() / io_times.len() as u32;
            
            println!("📊 tokio steady-state averages:");
            println!("   Connect: {:?}", avg_connect);
            println!("   I/O: {:?}", avg_io);
        });
        
        // Warmup connections
        for _ in 0..5 {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut stream = tokio::net::TcpStream::connect(addr).await?;
            let _ = stream.write_all(b"test").await;
            let mut buf = [0u8; 64];
            let _ = stream.read(&mut buf).await;
        }
        
        // Steady state connections
        for _ in 0..50 {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut stream = tokio::net::TcpStream::connect(addr).await?;
            stream.write_all(b"test").await?;
            let mut buffer = [0u8; 64];
            stream.read(&mut buffer).await?;
        }
        
        server.abort();
    }
    
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            steady_state_test("io_uring").await
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        tokio::runtime::Runtime::new()?.block_on(async {
            steady_state_test("tokio").await
        })
    }
}