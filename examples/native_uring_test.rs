//! Test native io_uring WebSocket implementation
//!
//! Run with: cargo run --example native_uring_test --features io-uring

use fastwebsockets::{Role, uring::UringWebSocket};

#[cfg(feature = "io-uring")]
async fn test_native_uring() -> Result<(), Box<dyn std::error::Error>> {
    use fastwebsockets::uring;
    
    println!("Testing native io_uring WebSocket implementation...");
    
    let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
    let addr = listener.local_addr()?;
    println!("Server listening on {}", addr);
    
    // Server task using native io_uring
    let server = tokio_uring::spawn(async move {
        let (stream, peer_addr) = listener.accept().await.unwrap();
        println!("Server: accepted connection from {}", peer_addr);
        
        // Extract the underlying tokio_uring stream for native operations
        let native_stream = stream.into_inner();
        let mut ws = UringWebSocket::new(native_stream, Role::Server);
        
        // Simple HTTP upgrade response for WebSocket
        let response = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: test\r\n\r\n";
        ws.write_frame_native(response.to_vec()).await.unwrap();
        println!("Server: sent WebSocket upgrade response");
        
        // Try to read a WebSocket frame
        match ws.read_frame_native().await {
            Ok(frame_data) => {
                println!("Server: received frame with {} bytes", frame_data.len());
                // Echo the frame back
                ws.write_frame_native(frame_data).await.unwrap();
            }
            Err(e) => println!("Server: error reading frame: {}", e),
        }
        
        println!("Server: completed native io_uring operations");
    });
    
    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    
    // Client using native io_uring
    let stream = uring::TcpStream::connect(addr).await?;
    println!("Client: connected");
    
    let native_stream = stream.into_inner();
    let mut client_ws = UringWebSocket::new(native_stream, Role::Client);
    
    // Send HTTP upgrade request
    let request = b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: test\r\n\r\n";
    client_ws.write_frame_native(request.to_vec()).await?;
    println!("Client: sent WebSocket upgrade request");
    
    // Read upgrade response
    let response = client_ws.read_frame_native().await?;
    println!("Client: received response with {} bytes", response.len());
    
    // Send a simple WebSocket frame (this is a basic text frame)
    let text_frame = b"\x81\x05Hello"; // FIN=1, opcode=1 (text), payload_len=5, payload="Hello"
    client_ws.write_frame_native(text_frame.to_vec()).await?;
    println!("Client: sent WebSocket text frame");
    
    // Read echo response
    let echo = client_ws.read_frame_native().await?;
    println!("Client: received echo with {} bytes", echo.len());
    
    server.await.unwrap();
    println!("Native io_uring WebSocket test completed successfully!");
    Ok(())
}

#[cfg(not(feature = "io-uring"))]
async fn test_native_uring() -> Result<(), Box<dyn std::error::Error>> {
    println!("io-uring feature not enabled");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            test_native_uring().await
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        tokio::runtime::Runtime::new()?.block_on(test_native_uring())
    }
}