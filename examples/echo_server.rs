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

use fastwebsockets::upgrade;
use fastwebsockets::OpCode;
use hyper::server::conn::Http;
use hyper::service::service_fn;
use hyper::Body;
use hyper::Request;
use hyper::Response;
use tokio::net::TcpListener;

async fn handle_client(
  fut: upgrade::UpgradeFut,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let mut ws = fastwebsockets::FragmentCollector::new(fut.await?);

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
async fn server_upgrade(
  mut req: Request<Body>,
) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
  let (response, fut) = upgrade::upgrade(&mut req)?;

  tokio::spawn(async move {
    if let Err(e) = tokio::task::unconstrained(handle_client(fut)).await {
      eprintln!("Error in websocket connection: {}", e);
    }
  });

  Ok(response)
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
