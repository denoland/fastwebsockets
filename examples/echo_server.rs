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

#[cfg(feature = "futures")]
use async_std::net::TcpListener;
use fastwebsockets::upgrade;
use fastwebsockets::OpCode;
use fastwebsockets::WebSocketError;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
#[cfg(not(feature = "futures"))]
use tokio::net::TcpListener;

async fn handle_client(fut: upgrade::UpgradeFut) -> Result<(), WebSocketError> {
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
  mut req: Request<Incoming>,
) -> Result<Response<Empty<Bytes>>, WebSocketError> {
  let (response, fut) = upgrade::upgrade(&mut req)?;

  #[cfg(not(feature = "futures"))]
  tokio::task::spawn(async move {
    if let Err(e) = tokio::task::unconstrained(handle_client(fut)).await {
      eprintln!("Error in websocket connection: {}", e);
    }
  });

  #[cfg(feature = "futures")]
  async_std::task::spawn(async move {
    if let Err(e) = handle_client(fut).await {
      eprintln!("Error in websocket connection: {}", e);
    }
  });
  Ok(response)
}

fn main() -> Result<(), WebSocketError> {
  #[cfg(feature = "futures")]
  {
    async_std::task::block_on(async move {
      let listener = TcpListener::bind("127.0.0.1:8080").await?;
      println!("Server started, listening on {}", "127.0.0.1:8080");
      loop {
        let (stream, _) = listener.accept().await?;
        println!("client connected");
        async_std::task::spawn(async move {
          let io = fastwebsockets::FuturesIo::new(stream);
          let conn_fut = http1::Builder::new()
            .serve_connection(io, service_fn(server_upgrade))
            .with_upgrades();
          if let Err(e) = conn_fut.await {
            println!("An error occured {:?}", e);
          }
        });
      }
    })
  }

  #[cfg(not(feature = "futures"))]
  {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .unwrap();

    rt.block_on(async move {
      let listener = TcpListener::bind("127.0.0.1:8080").await?;
      println!("Server started, listening on {}", "127.0.0.1:8080");
      loop {
        let (stream, _) = listener.accept().await?;
        println!("Client connected");
        tokio::spawn(async move {
          let io = hyper_util::rt::TokioIo::new(stream);
          let conn_fut = http1::Builder::new()
            .serve_connection(io, service_fn(server_upgrade))
            .with_upgrades();
          if let Err(e) = conn_fut.await {
            println!("An error occurred: {:?}", e);
          }
        });
      }
    })
  }
}
