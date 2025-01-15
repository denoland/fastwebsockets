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
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
use hyper::server::conn::http1;
use http_body_util::Empty;
use hyper::body::Bytes;
use tokio_rustls::rustls;
use tokio_rustls::rustls::Certificate;
use tokio_rustls::rustls::PrivateKey;
use tokio_rustls::TlsAcceptor;
use anyhow::Result;
use std::sync::Arc;
use monoio::io::IntoPollIo;

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
) -> Result<Response<Empty::<Bytes>>, WebSocketError> {
  let (response, fut) = upgrade::upgrade(&mut req)?;

  monoio::spawn(async move {
    if let Err(e) = handle_client(fut).await {
      eprintln!("Error in websocket connection: {}", e);
    }
  });

  Ok(response)
}

fn tls_acceptor() -> Result<TlsAcceptor> {
  static KEY: &[u8] = include_bytes!("./localhost.key");
  static CERT: &[u8] = include_bytes!("./localhost.crt");

  let mut keys: Vec<PrivateKey> =
    rustls_pemfile::pkcs8_private_keys(&mut &*KEY)
      .map(|mut certs| certs.drain(..).map(PrivateKey).collect())
      .unwrap();
  let certs: Vec<Certificate> = rustls_pemfile::certs(&mut &*CERT)
    .map(|mut certs| certs.drain(..).map(Certificate).collect())
    .unwrap();
  dbg!(&certs);
  let config = rustls::ServerConfig::builder()
    .with_safe_defaults()
    .with_no_client_auth()
    .with_single_cert(certs, keys.remove(0)).unwrap();
  Ok(TlsAcceptor::from(Arc::new(config)))
}

#[monoio::main]
async fn main() -> Result<()> {
  let listener = monoio::net::TcpListener::bind("127.0.0.1:8080").unwrap();
  let acceptor = tls_acceptor()?;
  println!("Server started, listening on 127.0.0.1:8080");
  loop {
    let (stream, _) = listener.accept().await?;
    let hyper_conn = HyperConnection(stream.into_poll_io()?);

    let acceptor = acceptor.clone();
    println!("Client connected");
    monoio::spawn(async move {
      match acceptor.accept(hyper_conn).await {
        Ok(stream) => {
          let io = hyper_util::rt::TokioIo::new(stream);
          let conn_fut = http1::Builder::new()
            .serve_connection(io, service_fn(server_upgrade))
            .with_upgrades();
          if let Err(e) = conn_fut.await {
            println!("An error occurred: {:?}", e);
          }
        }
        Err(e) => {
          println!("An error occurred: {:?}", e);
        }
      }
    });
  }
}

use std::pin::Pin;
struct HyperConnection(monoio::net::tcp::stream_poll::TcpStreamPoll);

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


#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for HyperConnection {}
