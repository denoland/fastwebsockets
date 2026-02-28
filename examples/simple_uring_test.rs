//! Simple test to verify io_uring integration works
//!
//! Run with: cargo run --example simple_uring_test --features io-uring

use fastwebsockets::{Frame, OpCode, Role, WebSocket, Payload};

#[cfg(feature = "io-uring")]
async fn test_uring() -> Result<(), Box<dyn std::error::Error>> {
    use fastwebsockets::uring;
    
    println!("Testing io_uring backend...");
    
    let listener = uring::TcpListener::bind("127.0.0.1:0".parse().unwrap())?;
    let addr = listener.local_addr()?;
    println!("Server listening on {}", addr);
    
    // Spawn server task  
    let server_handle = tokio_uring::spawn(async move {
        let (stream, peer_addr) = listener.accept().await.unwrap();
        println!("Accepted connection from {}", peer_addr);
        
        let mut ws = WebSocket::after_handshake(stream, Role::Server);
        ws.set_auto_close(true);
        ws.set_auto_pong(true);
        
        let frame = ws.read_frame().await.unwrap();
        println!("Server received frame with opcode: {:?}", frame.opcode);
        
        if frame.opcode == OpCode::Text {
            let echo_text = "Echo: ".to_owned() + std::str::from_utf8(&frame.payload).unwrap();
            ws.write_frame(Frame::text(Payload::Owned(echo_text.into_bytes()))).await.unwrap();
        }
        
        let close_frame = ws.read_frame().await.unwrap();
        println!("Server received close frame with opcode: {:?}", close_frame.opcode);
        
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    
    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    
    // Client connection
    let stream = uring::TcpStream::connect(addr).await?;
    println!("Client connected");
    
    let mut ws = WebSocket::after_handshake(stream, Role::Client);
    
    // Send a message
    ws.write_frame(Frame::text(Payload::Borrowed(b"Hello io_uring!"))).await?;
    println!("Client sent message");
    
    // Read echo
    let response = ws.read_frame().await?;
    println!("Client received frame with opcode: {:?}", response.opcode);
    if response.opcode == OpCode::Text {
        println!("Echo content: {}", std::str::from_utf8(&response.payload)?);
    }
    
    // Close connection
    ws.write_frame(Frame::close(1000, b"")).await?;
    println!("Client sent close");
    
    // Wait for server to finish
    server_handle.await.unwrap().unwrap();
    
    println!("Test completed successfully!");
    Ok(())
}

#[cfg(not(feature = "io-uring"))]
async fn test_uring() -> Result<(), Box<dyn std::error::Error>> {
    println!("io-uring feature not enabled. Falling back to regular tokio.");
    
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    println!("Server listening on {}", addr);
    
    // Same test logic but with regular tokio
    let server_handle = tokio::spawn(async move {
        let (stream, peer_addr) = listener.accept().await.unwrap();
        println!("Accepted connection from {}", peer_addr);
        
        let mut ws = WebSocket::after_handshake(stream, Role::Server);
        ws.set_auto_close(true);
        ws.set_auto_pong(true);
        
        let frame = ws.read_frame().await.unwrap();
        println!("Server received frame with opcode: {:?}", frame.opcode);
        
        if frame.opcode == OpCode::Text {
            let echo_text = "Echo: ".to_owned() + std::str::from_utf8(&frame.payload).unwrap();
            ws.write_frame(Frame::text(Payload::Owned(echo_text.into_bytes()))).await.unwrap();
        }
        
        let close_frame = ws.read_frame().await.unwrap();
        println!("Server received close frame with opcode: {:?}", close_frame.opcode);
        
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    
    let stream = tokio::net::TcpStream::connect(addr).await?;
    println!("Client connected");
    
    let mut ws = WebSocket::after_handshake(stream, Role::Client);
    
    ws.write_frame(Frame::text(Payload::Borrowed(b"Hello tokio!"))).await?;
    println!("Client sent message");
    
    let response = ws.read_frame().await?;
    println!("Client received frame with opcode: {:?}", response.opcode);
    if response.opcode == OpCode::Text {
        println!("Echo content: {}", std::str::from_utf8(&response.payload)?);
    }
    
    ws.write_frame(Frame::close(1000, b"")).await?;
    println!("Client sent close");
    
    server_handle.await.unwrap().unwrap();
    
    println!("Test completed successfully!");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "io-uring")]
    {
        fastwebsockets::uring::start(async {
            test_uring().await
        })
    }
    
    #[cfg(not(feature = "io-uring"))]
    {
        tokio::runtime::Runtime::new()?.block_on(test_uring())
    }
}