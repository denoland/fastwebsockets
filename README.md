_fastwebsockets_ is a fast WebSocket server implementation.

Passes the
Autobahn|TestSuite<sup><a href="https://littledivy.github.io/fastwebsockets/servers/">1</a></sup>
and fuzzed with LLVM's libfuzzer.

You can use it as a raw websocket frame parser and deal with spec compliance
yourself, or you can use it as a full-fledged websocket server.

```rust
use fastwebsockets::{Frame, OpCode, WebSocket};

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

**Fragmentation**

By default, fastwebsockets will give the application raw frames with FIN set. Other
crates like tungstenite which will give you a single message with all the frames
concatenated.

For concanated frames, use `FragmentCollector`:

```rust
let mut ws = WebSocket::after_handshake(socket);
let mut ws = FragmentCollector::new(ws);

let incoming = ws.read_frame().await?;
// Always returns full messages
assert!(incoming.fin);
```

> permessage-deflate is not supported yet.
