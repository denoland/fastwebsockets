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
use fastwebsockets::OpCode;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::rustls;
use tokio_rustls::rustls::Certificate;
use tokio_rustls::rustls::PrivateKey;
use tokio_rustls::TlsAcceptor;

async fn handle_client(fut: upgrade::UpgradeFut) -> Result<()> {
  let mut ws = fut.await?;
  ws.set_writev(false);
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

async fn server_upgrade(
  mut req: Request<Incoming>,
) -> Result<Response<Empty<Bytes>>> {
  let (response, fut) = upgrade::upgrade(&mut req)?;

  tokio::spawn(async move {
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
  let certs = rustls_pemfile::certs(&mut &*CERT)
    .map(|mut certs| certs.drain(..).map(Certificate).collect())
    .unwrap();
  dbg!(&certs);
  let config = rustls::ServerConfig::builder()
    .with_safe_defaults()
    .with_no_client_auth()
    .with_single_cert(certs, keys.remove(0))?;
  Ok(TlsAcceptor::from(Arc::new(config)))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
  let acceptor = tls_acceptor()?;
  let listener = TcpListener::bind("127.0.0.1:8080").await?;
  println!("Server started, listening on {}", "127.0.0.1:8080");
  loop {
    let (stream, _) = listener.accept().await?;
    println!("Client connected");
    let acceptor = acceptor.clone();
    tokio::spawn(async move {
      let stream = acceptor.accept(stream).await.unwrap();
      let io = hyper_util::rt::TokioIo::new(stream);
      let conn_fut = http1::Builder::new()
        .serve_connection(io, service_fn(server_upgrade))
        .with_upgrades();
      if let Err(e) = conn_fut.await {
        println!("An error occurred: {:?}", e);
      }
    });
  }
}
