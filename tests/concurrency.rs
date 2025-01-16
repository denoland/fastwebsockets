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

use anyhow::Result;
use fastwebsockets::upgrade;
use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use fastwebsockets::handshake;
use fastwebsockets::WebSocket;
use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::upgrade::Upgraded;
use std::future::Future;
use tokio::net::TcpStream;

const N_CLIENTS: usize = 20;

async fn handle_client(
  client_id: usize,
  fut: upgrade::UpgradeFut,
) -> Result<()> {
  let mut ws = fut.await?;
  ws.set_writev(false);
  let mut ws = fastwebsockets::FragmentCollector::new(ws);

  ws.write_frame(Frame::binary(client_id.to_ne_bytes().as_ref().into()))
    .await
    .unwrap();

  Ok(())
}

async fn server_upgrade(
  mut req: Request<Incoming>,
) -> Result<Response<Empty<Bytes>>> {
  let (response, fut) = upgrade::upgrade(&mut req)?;

  let client_id: usize = req
    .headers()
    .get("CLIENT-ID")
    .unwrap()
    .to_str()
    .unwrap()
    .parse()
    .unwrap();
  tokio::spawn(async move {
    handle_client(client_id, fut).await.unwrap();
  });

  Ok(response)
}

async fn connect(client_id: usize) -> Result<WebSocket<TokioIo<Upgraded>>> {
  let stream = TcpStream::connect("localhost:8080").await?;

  let req = Request::builder()
    .method("GET")
    .uri("http://localhost:8080/")
    .header("Host", "localhost:8080")
    .header(UPGRADE, "websocket")
    .header(CONNECTION, "upgrade")
    .header("CLIENT-ID", &format!("{}", client_id))
    .header(
      "Sec-WebSocket-Key",
      fastwebsockets::handshake::generate_key(),
    )
    .header("Sec-WebSocket-Version", "13")
    .body(Empty::<Bytes>::new())?;

  let (ws, _) = handshake::client(&SpawnExecutor, req, stream).await?;
  Ok(ws)
}

async fn start_client(client_id: usize) -> Result<()> {
  let mut ws = connect(client_id).await.unwrap();
  let frame = ws.read_frame().await?;
  match frame.opcode {
    OpCode::Close => {}
    OpCode::Binary => {
      let n = usize::from_ne_bytes(frame.payload[..].try_into().unwrap());
      assert_eq!(n, client_id);
    }
    _ => {
      panic!("Unexpected");
    }
  }
  Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test() -> Result<()> {
  let listener = TcpListener::bind("127.0.0.1:8080").await?;
  println!("Server started, listening on 127.0.0.1:8080");
  tokio::spawn(async move {
    loop {
      let (stream, _) = listener.accept().await.unwrap();
      tokio::spawn(async move {
        let io = TokioIo::new(stream);
        let conn_fut = http1::Builder::new()
          .serve_connection(io, service_fn(server_upgrade))
          .with_upgrades();
        conn_fut.await.unwrap();
      });
    }
  });
  let mut tasks = Vec::with_capacity(N_CLIENTS);
  for client in 0..N_CLIENTS {
    tasks.push(tokio::spawn(start_client(client)));
  }
  for handle in tasks {
    handle.await.unwrap().unwrap();
  }
  Ok(())
}

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