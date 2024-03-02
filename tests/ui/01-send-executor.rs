use fastwebsockets::WebSocket;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::upgrade::Upgraded;
use hyper::Request;
use std::future::Future;

use anyhow::Result;
#[cfg(feature = "futures")]
use async_std::{net::TcpStream, task::spawn};
#[cfg(feature = "futures")]
use fastwebsockets::FuturesIo as IoWrapper;
#[cfg(not(feature = "futures"))]
use hyper_util::rt::tokio::TokioIo as IoWrapper;
#[cfg(not(feature = "futures"))]
use tokio::{net::TcpStream, spawn};

struct SpawnExecutor;

impl<Fut> hyper::rt::Executor<Fut> for SpawnExecutor
where
  Fut: Future + Send + 'static,
  Fut::Output: Send + 'static,
{
  fn execute(&self, fut: Fut) {
    spawn(fut);
  }
}

async fn connect(path: &str) -> Result<WebSocket<IoWrapper<Upgraded>>> {
  let stream = TcpStream::connect("localhost:9001").await?;

  let req = Request::builder()
    .method("GET")
    .uri(format!("http://localhost:9001/{}", path))
    .header("Host", "localhost:9001")
    .header(UPGRADE, "websocket")
    .header(CONNECTION, "upgrade")
    .header(
      "Sec-WebSocket-Key",
      fastwebsockets::handshake::generate_key(),
    )
    .header("Sec-WebSocket-Version", "13")
    .body(Empty::<Bytes>::new())?;

  let (ws, _) =
    fastwebsockets::handshake::client(&SpawnExecutor, req, stream).await?;
  Ok(ws)
}

fn main() {}
