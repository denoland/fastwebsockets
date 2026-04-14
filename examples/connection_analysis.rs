//! Analyze connection establishment performance
//!
//! Run tokio: cargo run --example connection_analysis --release
//! Run io_uring: cargo run --example connection_analysis --release --features io-uring

use std::time::Instant;

async fn detailed_connection_test(backend: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== {} Connection Analysis ===", backend);
    
    #[cfg(feature = "io-uring")]
    {
        use fastwebsockets::uring;
        
        println!("🔍 Analyzing io_uring connection overhead...");
        
        // Test 1: Just listener creation
        let listener_start = Instant::now();
        let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
        let listener_time = listener_start.elapsed();
        let addr = listener.local_addr()?;
        println!("⏱️  io_uring listener creation: {:?}", listener_time);
        
        // Test 2: Connection establishment only
        let server = tokio_uring::spawn(async move {
            let accept_start = Instant::now();
            let (_stream, _peer) = listener.accept().await.unwrap();
            let accept_time = accept_start.elapsed();
            println!("⏱️  io_uring accept time: {:?}", accept_time);
        });
        
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        
        let connect_start = Instant::now();
        let _stream = uring::TcpStream::connect(addr).await?;
        let connect_time = connect_start.elapsed();
        println!("⏱️  io_uring connect time: {:?}", connect_time);
        
        server.await.unwrap();
        
        // Test 3: Multiple connections to see if there's warmup overhead
        println!("\n🔥 Testing connection warmup (10 connections):");
        let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
        let addr = listener.local_addr()?;
        
        let server = tokio_uring::spawn(async move {
            for i in 0..10 {
                let start = Instant::now();
                let (_stream, _) = listener.accept().await.unwrap();
                println!("  Accept #{}: {:?}", i+1, start.elapsed());
            }
        });
        
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        
        for i in 0..10 {
            let start = Instant::now();
            let _stream = uring::TcpStream::connect(addr).await?;
            println!("  Connect #{}: {:?}", i+1, start.elapsed());
        }
        
        server.await.unwrap();
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        println!("🔍 Analyzing tokio connection performance...");
        
        // Test 1: Just listener creation
        let listener_start = Instant::now();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let listener_time = listener_start.elapsed();
        let addr = listener.local_addr()?;
        println!("⏱️  tokio listener creation: {:?}", listener_time);
        
        // Test 2: Connection establishment only
        let server = tokio::spawn(async move {
            let accept_start = Instant::now();
            let (_stream, _peer) = listener.accept().await.unwrap();
            let accept_time = accept_start.elapsed();
            println!("⏱️  tokio accept time: {:?}", accept_time);
        });
        
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        
        let connect_start = Instant::now();
        let _stream = tokio::net::TcpStream::connect(addr).await?;
        let connect_time = connect_start.elapsed();
        println!("⏱️  tokio connect time: {:?}", connect_time);
        
        server.abort();
        
        // Test 3: Multiple connections
        println!("\n🔥 Testing connection warmup (10 connections):");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        
        let server = tokio::spawn(async move {
            for i in 0..10 {
                let start = Instant::now();
                let (_stream, _) = listener.accept().await.unwrap();
                println!("  Accept #{}: {:?}", i+1, start.elapsed());
            }
        });
        
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        
        for i in 0..10 {
            let start = Instant::now();
            let _stream = tokio::net::TcpStream::connect(addr).await?;
            println!("  Connect #{}: {:?}", i+1, start.elapsed());
        }
        
        server.abort();
    }
    
    Ok(())
}

async fn runtime_overhead_test() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🚀 Runtime Overhead Analysis");
    println!("=============================");
    
    #[cfg(feature = "io-uring")]
    {
        println!("🧪 io_uring runtime characteristics:");
        println!("  • Uses tokio_uring::start() instead of tokio::Runtime");
        println!("  • Single-threaded io_uring executor");
        println!("  • Different task scheduling model");
        println!("  • Potential cold start overhead");
        println!("\n💡 Connection slowness likely due to:");
        println!("  1. Runtime initialization overhead");
        println!("  2. io_uring submission queue setup");
        println!("  3. Kernel driver initialization");
        println!("  4. Different connection pooling strategy");
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        println!("🧪 tokio runtime characteristics:");
        println!("  • Multi-threaded work-stealing scheduler");
        println!("  • Mature connection handling");
        println!("  • Optimized for connection establishment");
        println!("  • Well-tuned epoll integration");
    }
    
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            detailed_connection_test("io_uring").await?;
            runtime_overhead_test().await
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        tokio::runtime::Runtime::new()?.block_on(async {
            detailed_connection_test("tokio").await?;
            runtime_overhead_test().await
        })
    }
}