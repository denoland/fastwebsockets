//! Minimal test using io_uring directly without AsyncRead/AsyncWrite
//!
//! Run with: cargo run --example minimal_uring --features io-uring

use fastwebsockets::{Frame, OpCode, Role, WebSocket, Payload};

#[cfg(feature = "io-uring")]
async fn test_direct() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing direct tokio-uring API (bypassing AsyncRead/AsyncWrite)...");
    
    let listener = tokio_uring::net::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
    let addr = listener.local_addr()?;
    println!("Listening on {}", addr);
    
    // Server task
    let server = tokio_uring::spawn(async move {
        let (stream, peer) = listener.accept().await.unwrap();
        println!("Server: accepted connection from {}", peer);
        
        // Direct io_uring API test
        let buf = vec![0u8; 1024];
        let (result, buf) = stream.read(buf).await;
        let n = result.unwrap();
        println!("Server: read {} bytes", n);
        
        let response = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: test\r\n\r\n";
        let (result, _) = stream.write_all(response.to_vec()).await;
        result.unwrap();
        println!("Server: sent HTTP response");
        
        // Now we need to use our wrapper for WebSocket operations
        let wrapped_stream = fastwebsockets::uring::TcpStream::from_std(stream.into_std().unwrap());
        let mut ws = WebSocket::after_handshake(wrapped_stream, Role::Server);
        ws.set_auto_close(true);
        ws.set_auto_pong(true);
        
        // This will test the AsyncRead/AsyncWrite adapter
        let frame = ws.read_frame().await.unwrap();
        println!("Server: received websocket frame with opcode {:?}", frame.opcode);
        
        if frame.opcode == OpCode::Text {
            ws.write_frame(frame).await.unwrap(); // Echo
        }
        
        println!("Server: completed");
    });
    
    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    
    // Client
    let stream = tokio_uring::net::TcpStream::connect(addr).await?;
    println!("Client: connected");
    
    let request = b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    let (result, _) = stream.write_all(request.to_vec()).await;
    result?;
    println!("Client: sent HTTP request");
    
    let buf = vec![0u8; 1024];
    let (result, buf) = stream.read(buf).await;
    let n = result?;
    println!("Client: received {} bytes of HTTP response", n);
    
    // Convert to our wrapper for WebSocket operations
    let wrapped_stream = fastwebsockets::uring::TcpStream::from_std(stream.into_std().unwrap());
    let mut ws = WebSocket::after_handshake(wrapped_stream, Role::Client);
    
    ws.write_frame(Frame::text(Payload::Borrowed(b"Hello io_uring!"))).await?;
    println!("Client: sent websocket message");
    
    let response = ws.read_frame().await?;
    println!("Client: received echo: {:?}", std::str::from_utf8(&response.payload));
    
    server.await.unwrap();
    println!("Test completed successfully!");
    Ok(())
}

#[cfg(not(feature = "io-uring"))]
async fn test_direct() -> Result<(), Box<dyn std::error::Error>> {
    println!("io-uring feature not enabled");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            test_direct().await
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        tokio::runtime::Runtime::new()?.block_on(test_direct())
    }
}