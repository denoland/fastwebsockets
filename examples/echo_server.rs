// Copyright 2023 Divy Srivastava <dj.srivastava23@gmail.com>
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

use base64;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use fastwebsockets::OpCode;
use fastwebsockets::WebSocket;
use sha1::Digest;
use sha1::Sha1;
use tokio::net::TcpListener;

use hyper::header::HeaderValue;
use hyper::header::UPGRADE;
use hyper::server::conn::Http;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::Body;
use hyper::Request;
use hyper::Response;
use hyper::StatusCode;

async fn handle_client(
  socket: Upgraded,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let mut ws = WebSocket::after_handshake(socket);
  ws.set_writev(true);
  ws.set_auto_close(true);
  ws.set_auto_pong(true);

  let mut ws = fastwebsockets::FragmentCollector::new(ws);

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

fn sec_websocket_protocol(key: &[u8]) -> String {
  let mut sha1 = Sha1::new();
  sha1.update(key);
  sha1.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"); // magic string
  let result = sha1.finalize();
  STANDARD.encode(&result[..])
}

async fn server_upgrade(
  mut req: Request<Body>,
) -> Result<Response<String>, Box<dyn std::error::Error + Send + Sync>> {
  let mut res = Response::new(String::new());
  if !req.headers().contains_key(UPGRADE) {
    *res.status_mut() = StatusCode::BAD_REQUEST;
    return Ok(res);
  }

  *res.status_mut() = StatusCode::SWITCHING_PROTOCOLS;

  if let Some(key) = req.headers().get("Sec-WebSocket-Protocol") {
    res.headers_mut().insert(
      "Sec-WebSocket-Protocol",
      HeaderValue::from_str(&sec_websocket_protocol(key.as_bytes()))?,
    );
  }

  tokio::task::spawn(async move {
    match hyper::upgrade::on(&mut req).await {
      Ok(upgraded) => {
        if let Err(e) = handle_client(upgraded).await {
          eprintln!("io error: {}", e)
        };
      }
      Err(e) => eprintln!("upgrade error: {}", e),
    }
  });

  Ok(res)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let listener = TcpListener::bind("127.0.0.1:8080").await?;
  println!("Server started, listening on {}", "127.0.0.1:8080");
  loop {
    let (stream, _) = listener.accept().await?;
    println!("Client connected");
    tokio::spawn(async move {
      let conn_fut = Http::new()
        .serve_connection(stream, service_fn(server_upgrade))
        .with_upgrades();
      if let Err(e) = conn_fut.await {
        println!("An error occurred: {:?}", e);
      }
    });
  }
}
