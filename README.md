[![Crates.io](https://img.shields.io/crates/v/fastwebsockets.svg)](https://crates.io/crates/fastwebsockets)

[Documentation](https://docs.rs/fastwebsockets)

_fastwebsockets_ is a fast WebSocket protocol implementation.

Passes the
Autobahn|TestSuite<sup><a href="https://littledivy.github.io/fastwebsockets/servers/">1</a></sup>
and fuzzed with LLVM's libfuzzer.

You can use it as a raw websocket frame parser and deal with spec compliance
yourself, or you can use it as a full-fledged websocket client/server.

```rust
use fastwebsockets::{Frame, OpCode, WebSocket};

async fn handle_client(
  mut socket: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  handshake(&mut socket).await?;

  let mut ws = WebSocket::after_handshake(socket);
  ws.set_writev(true);
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

By default, fastwebsockets will give the application raw frames with FIN set.
Other crates like tungstenite which will give you a single message with all the
frames concatenated.

For concanated frames, use `FragmentCollector`:

```rust
let mut ws = WebSocket::after_handshake(socket);
let mut ws = FragmentCollector::new(ws);

let incoming = ws.read_frame().await?;
// Always returns full messages
assert!(incoming.fin);
```

> permessage-deflate is not supported yet.

**HTTP Upgrade**

Enable the `upgrade` feature to do server-side upgrades and client-side
handshakes.

This feature is powered by [hyper](https://docs.rs/hyper).

```rust
use fastwebsockets::upgrade::upgrade;
use hyper::{Request, Body, Response};

async fn server_upgrade(
  mut req: Request<Body>,
) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
  let (response, fut) = upgrade::upgrade(&mut req)?;

  tokio::spawn(async move {
    if let Err(e) = handle_client(fut).await {
      eprintln!("Error in websocket connection: {}", e);
    }
  });

  Ok(response)
}
```

Use the `handshake` module for client-side handshakes.

```rust
use fastwebsockets::handshake;
use fastwebsockets::WebSocket;
use hyper::{Request, Body, upgrade::Upgraded, header::{UPGRADE, CONNECTION}};
use tokio::net::TcpStream;
use std::future::Future;

// Define a type alias for convenience
type Result<T> =
  std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn connect() -> Result<WebSocket<Upgraded>> {
  let stream = TcpStream::connect("localhost:9001").await?;

  let req = Request::builder()
    .method("GET")
    .uri("http://localhost:9001/")
    .header("Host", "localhost:9001")
    .header(UPGRADE, "websocket")
    .header(CONNECTION, "upgrade")
    .header(
      "Sec-WebSocket-Key",
      fastwebsockets::handshake::generate_key(),
    )
    .header("Sec-WebSocket-Version", "13")
    .body(Body::empty())?;

  let (ws, _) = handshake::client(&SpawnExecutor, req, stream).await?;
  Ok(ws)
}

// Tie hyper's executor to tokio runtime
struct SpawnExecutor;

impl<Fut> hyper::rt::Executor<Fut> for SpawnExecutor
where
  Fut: Future + Send + 'static,
  Fut::Output: Send + 'static,
{
  fn execute(&self, fut: Fut) {
    tokio::task::spawn(fut);
  }
}
```
