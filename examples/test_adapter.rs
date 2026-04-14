//! Test the io_uring adapter in isolation
//!
//! Run with: cargo run --example test_adapter --features io-uring

use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cfg(feature = "io-uring")]
async fn test_adapter() -> Result<(), Box<dyn std::error::Error>> {
    use fastwebsockets::uring;
    
    println!("Testing io_uring adapter directly...");
    
    let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
    let addr = listener.local_addr()?;
    println!("Listening on {}", addr);
    
    // Server
    let server = tokio_uring::spawn(async move {
        let (mut stream, peer) = listener.accept().await.unwrap();
        println!("Server: accepted connection from {}", peer);
        
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        println!("Server: read {} bytes: {:?}", n, std::str::from_utf8(&buf[..n]));
        
        stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n").await.unwrap();
        println!("Server: wrote response");
    });
    
    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    
    // Client
    let mut stream = uring::TcpStream::connect(addr).await?;
    println!("Client: connected");
    
    stream.write_all(b"GET / HTTP/1.1\r\nConnection: upgrade\r\n\r\n").await?;
    println!("Client: wrote request");
    
    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await?;
    println!("Client: read {} bytes: {:?}", n, std::str::from_utf8(&buf[..n]));
    
    server.await.unwrap();
    println!("Test completed successfully!");
    Ok(())
}

#[cfg(not(feature = "io-uring"))]
async fn test_adapter() -> Result<(), Box<dyn std::error::Error>> {
    println!("io-uring feature not enabled");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            test_adapter().await
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        tokio::runtime::Runtime::new()?.block_on(test_adapter())
    }
}