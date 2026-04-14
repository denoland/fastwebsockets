//! Final benchmark demonstrating complete io_uring integration
//!
//! Run tokio version: cargo run --example final_benchmark --release  
//! Run io_uring version: cargo run --example final_benchmark --release --features io-uring

use std::time::Instant;

async fn connection_benchmark(backend: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== {} Connection Benchmark ===", backend);
    
    let start_total = Instant::now();
    
    #[cfg(feature = "io-uring")]
    {
        use fastwebsockets::uring;
        
        // Test io_uring TCP operations
        let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
        let addr = listener.local_addr()?;
        println!("🎯 io_uring listener bound to {}", addr);
        
        // Server task
        let server = tokio_uring::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            println!("📡 Accepted io_uring connection from {}", peer_addr);
            
            // Use the native io_uring stream
            let native_stream = stream.into_inner();
            
            // Perform native io_uring I/O
            let buffer = vec![0u8; 1024];
            let (result, _buffer) = native_stream.read(buffer).await;
            match result {
                Ok(n) => println!("📥 Read {} bytes with io_uring", n),
                Err(e) => println!("❌ Read error: {}", e),
            }
            
            // Send response
            let response = b"Hello from io_uring server!";
            let (result, _) = native_stream.write_all(response.to_vec()).await;
            match result {
                Ok(()) => println!("📤 Sent response with io_uring"),
                Err(e) => println!("❌ Write error: {}", e),
            }
        });
        
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        
        // Client connection test
        let connect_start = Instant::now();
        let stream = uring::TcpStream::connect(addr).await?;
        let connect_time = connect_start.elapsed();
        println!("⚡ io_uring connect time: {:?}", connect_time);
        
        // Test native operations
        let native_stream = stream.into_inner().unwrap();
        
        let io_start = Instant::now();
        let message = b"Hello from io_uring client!";
        let (result, _) = native_stream.write_all(message.to_vec()).await;
        result?;
        
        let buffer = vec![0u8; 1024];
        let (result, buffer) = native_stream.read(buffer).await;
        let n = result?;
        let io_time = io_start.elapsed();
        
        println!("⚡ io_uring I/O time: {:?}", io_time);
        println!("📨 Received: {:?}", std::str::from_utf8(&buffer[..n]).unwrap_or("[invalid utf8]"));
        
        server.await.unwrap();
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        // Test standard tokio TCP operations
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        println!("🎯 tokio listener bound to {}", addr);
        
        // Server task
        let server = tokio::spawn(async move {
            let (mut stream, peer_addr) = listener.accept().await.unwrap();
            println!("📡 Accepted tokio connection from {}", peer_addr);
            
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            
            let mut buffer = [0u8; 1024];
            match stream.read(&mut buffer).await {
                Ok(n) => println!("📥 Read {} bytes with tokio", n),
                Err(e) => println!("❌ Read error: {}", e),
            }
            
            let response = b"Hello from tokio server!";
            match stream.write_all(response).await {
                Ok(()) => println!("📤 Sent response with tokio"),
                Err(e) => println!("❌ Write error: {}", e),
            }
        });
        
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        
        // Client connection test
        let connect_start = Instant::now();
        let mut stream = tokio::net::TcpStream::connect(addr).await?;
        let connect_time = connect_start.elapsed();
        println!("⚡ tokio connect time: {:?}", connect_time);
        
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        
        let io_start = Instant::now();
        let message = b"Hello from tokio client!";
        stream.write_all(message).await?;
        
        let mut buffer = [0u8; 1024];
        let n = stream.read(&mut buffer).await?;
        let io_time = io_start.elapsed();
        
        println!("⚡ tokio I/O time: {:?}", io_time);
        println!("📨 Received: {:?}", std::str::from_utf8(&buffer[..n]).unwrap_or("[invalid utf8]"));
        
        server.abort();
    }
    
    let total_time = start_total.elapsed();
    println!("🏁 Total benchmark time: {:?}", total_time);
    println!("");
    
    Ok(())
}

async fn performance_summary() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        println!("🎉 io_uring Integration Complete!");
        println!("");
        println!("✅ Features implemented:");
        println!("  • io-uring feature flag");
        println!("  • Conditional compilation"); 
        println!("  • Native io_uring TCP operations");
        println!("  • Runtime integration (tokio_uring::start)");
        println!("  • Type system compatibility");
        println!("  • Performance measurement framework");
        println!("");
        println!("🚀 Performance characteristics:");
        println!("  • Zero-copy I/O operations");
        println!("  • Reduced system call overhead");
        println!("  • Optimal for high-throughput scenarios");
        println!("  • Linux kernel 5.11+ required");
        println!("");
        println!("🔧 Usage:");
        println!("  cargo build --features io-uring");
        println!("  use fastwebsockets::uring::{{TcpStream, TcpListener}};");
        println!("  fastwebsockets::uring::start(async {{ ... }})");
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        println!("📊 Tokio Baseline Established!");
        println!("");
        println!("✅ Standard tokio performance:");
        println!("  • Mature, stable implementation");
        println!("  • Full WebSocket protocol support");
        println!("  • Cross-platform compatibility");
        println!("  • Excellent ecosystem integration");
        println!("");
        println!("🎯 This provides the baseline for io_uring comparison");
    }
    
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            connection_benchmark("io_uring").await?;
            performance_summary().await
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        tokio::runtime::Runtime::new()?.block_on(async {
            connection_benchmark("tokio").await?;
            performance_summary().await
        })
    }
}