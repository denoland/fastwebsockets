_sockdeez_ is a fast WebSocket server implementation.

You can use it as a raw websocket frame parser and deal with spec compliance
yourself, or you can use it as a full-fledged websocket server.

```rust
use sockdeez::{Frame, OpCode, WebSocket};

async fn handle_client(
  mut socket: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  handshake(&mut socket).await?;

  let mut ws = WebSocket::after_handshake(socket);
  ws.set_writev(false);
  ws.set_auto_close(true);
  ws.set_auto_pong(true);

  loop {
    let frame = ws.read_frame().await?;

    match frame {
      OpCode::Close => break,
      OpCode::Text | OpCode::Binary => {
        let frame = Frame::new(true, frame.opcode, None, frame.payload);
        ws.write_frame(frame).await?;
      }
    }
  }

  Ok(())
}
```
