use fastwebsockets::WebSocket;
use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::upgrade::Upgraded;
use hyper::Body;
use hyper::Request;
use std::future::Future;
use tokio::net::TcpStream;

struct SpawnLocalExecutor;

impl<Fut> hyper::rt::Executor<Fut> for SpawnLocalExecutor
where
  Fut: Future + 'static,
  Fut::Output: 'static,
{
  fn execute(&self, fut: Fut) {
    tokio::task::spawn_local(fut);
  }
}

async fn connect(
  path: &str,
) -> Result<WebSocket<Upgraded>, Box<dyn std::error::Error + Send + Sync>> {
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
    .body(Body::empty())?;

  let (ws, _) =
    fastwebsockets::handshake::client(&SpawnLocalExecutor, req, stream).await?;
  Ok(ws)
}

fn main() {}
