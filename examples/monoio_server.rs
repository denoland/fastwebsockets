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
use fastwebsockets::WebSocketError;
use hyper::server::conn::Http;
use hyper::service::service_fn;
use hyper::Body;
use hyper::Request;
use hyper::Response;
use monoio_compat::AsyncReadExt;
use monoio_compat::AsyncWriteExt;
use monoio_compat::TcpStreamCompat;
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
  mut req: Request<Body>,
) -> Result<Response<Body>, WebSocketError> {
  let (response, fut) = upgrade::upgrade(&mut req)?;

  monoio::spawn(async move {
    if let Err(e) = handle_client(fut).await {
      eprintln!("Error in websocket connection: {}", e);
    }
  });

  Ok(response)
}

#[monoio::main]
async fn main() -> Result<(), WebSocketError> {
  let listener = monoio::net::TcpListener::bind("127.0.0.1:8080").unwrap();

  println!("Server started, listening on {}", "127.0.0.1:8080");
  loop {
    let (stream, _) = listener.accept().await?;

    let hyper_conn = HyperConnection(stream);
    println!("Client connected");
    monoio::spawn(async move {
      let conn_fut = Http::new()
        .with_executor(HyperExecutor)
        .serve_connection(hyper_conn, service_fn(server_upgrade))
        .with_upgrades();
      if let Err(e) = conn_fut.await {
        println!("An error occurred: {:?}", e);
      }
    });
  }
}

use std::pin::Pin;
struct HyperConnection(monoio::net::TcpStream);

impl tokio::io::AsyncRead for HyperConnection {
  #[inline]
  fn poll_read(
    mut self: Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
    buf: &mut tokio::io::ReadBuf<'_>,
  ) -> std::task::Poll<std::io::Result<()>> {
    Pin::new(&mut self.0).poll_read(cx, buf)
  }
}

impl tokio::io::AsyncWrite for HyperConnection {
  #[inline]
  fn poll_write(
    mut self: Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
    buf: &[u8],
  ) -> std::task::Poll<Result<usize, std::io::Error>> {
    Pin::new(&mut self.0).poll_write(cx, buf)
  }

  #[inline]
  fn poll_flush(
    mut self: Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<Result<(), std::io::Error>> {
    Pin::new(&mut self.0).poll_flush(cx)
  }

  #[inline]
  fn poll_shutdown(
    mut self: Pin<&mut Self>,
    cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<Result<(), std::io::Error>> {
    Pin::new(&mut self.0).poll_shutdown(cx)
  }
}

impl hyper::client::connect::Connection for HyperConnection {
  #[inline]
  fn connected(&self) -> hyper::client::connect::Connected {
    hyper::client::connect::Connected::new()
  }
}

#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for HyperConnection {}

use std::future::Future;
#[derive(Clone)]
struct HyperExecutor;

impl<F> hyper::rt::Executor<F> for HyperExecutor
where
  F: Future + 'static,
  F::Output: 'static,
{
  fn execute(&self, fut: F) {
    monoio::spawn(fut);
  }
}
