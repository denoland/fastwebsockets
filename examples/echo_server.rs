// Copyright 2023-2026 Divy Srivastava <dj.srivastava23@gmail.com>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use fastwebsockets::upgrade;
use fastwebsockets::FragmentCollector;
use fastwebsockets::OpCode;
use fastwebsockets::Role;
use fastwebsockets::WebSocket;
use fastwebsockets::WebSocketError;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

async fn echo_loop<S>(ws: WebSocket<S>) -> Result<(), WebSocketError>
where
  S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
  // The bench load_test.c never fragments, but the Autobahn suite does and
  // expects cross-fragment UTF-8 validation. Wrap with FragmentCollector so
  // the example stays protocol-compliant; FragmentCollector is a thin
  // pass-through for non-fragmented frames (one match per frame).
  let mut ws = FragmentCollector::new(ws);
  loop {
    let frame = ws.read_frame().await?;
    match frame.opcode {
      OpCode::Close => break,
      OpCode::Text | OpCode::Binary => {
        ws.write_frame(frame).await?;
      }
      _ => {}
    }
  }
  Ok(())
}

async fn handle_client(fut: upgrade::UpgradeFut) -> Result<(), WebSocketError> {
  // Drive hyper's upgrade future, then downcast to the underlying TcpStream so
  // the steady-state echo loop runs without hyper's read-buffer + trait-object
  // indirection on every read/write.
  let upgraded = fut.upgraded().await?;
  match upgraded.downcast::<TokioIo<TcpStream>>() {
    Ok(parts) => {
      // hyper may have buffered bytes the client sent right after the upgrade
      // request. Carry them into the WebSocket's framing buffer.
      let stream = parts.io.into_inner();
      let _ = stream.set_nodelay(true);
      let ws = WebSocket::after_handshake_with_buffer(
        stream,
        Role::Server,
        &parts.read_buf,
      );
      echo_loop(ws).await
    }
    Err(upgraded) => {
      // Some other transport (TLS, h2c) — fall back to the generic path.
      let ws = WebSocket::after_handshake(TokioIo::new(upgraded), Role::Server);
      echo_loop(ws).await
    }
  }
}

async fn handle_client_tcp(stream: TcpStream) -> Result<(), WebSocketError> {
  let _ = stream.set_nodelay(true);
  let io = TokioIo::new(stream);
  let conn_fut = http1::Builder::new()
    .serve_connection(io, service_fn(server_upgrade))
    .with_upgrades();
  if let Err(e) = conn_fut.await {
    eprintln!("An error occurred: {:?}", e);
  }
  Ok(())
}

async fn server_upgrade(
  mut req: Request<Incoming>,
) -> Result<Response<Empty<Bytes>>, WebSocketError> {
  let (response, fut) = upgrade::upgrade(&mut req)?;

  tokio::task::spawn(async move {
    if let Err(e) = tokio::task::unconstrained(handle_client(fut)).await {
      eprintln!("Error in websocket connection: {}", e);
    }
  });

  Ok(response)
}

fn main() -> Result<(), WebSocketError> {
  let workers = std::env::var("FWS_WORKERS")
    .ok()
    .and_then(|s| s.parse::<usize>().ok())
    .unwrap_or(1);

  let mut builder = if workers <= 1 {
    tokio::runtime::Builder::new_current_thread()
  } else {
    let mut b = tokio::runtime::Builder::new_multi_thread();
    b.worker_threads(workers);
    b
  };
  let rt = builder.enable_io().build().unwrap();

  let addr =
    std::env::var("FWS_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

  rt.block_on(async move {
    let listener = TcpListener::bind(&addr).await?;
    println!("Server started, listening on {}", addr);
    loop {
      let (stream, _) = listener.accept().await?;
      tokio::spawn(async move {
        if let Err(e) = handle_client_tcp(stream).await {
          eprintln!("connection error: {}", e);
        }
      });
    }
  })
}
