use fastwebsockets::WebSocket;
use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::upgrade::Upgraded;
use hyper::Body;
use hyper::Request;
use std::future::Future;
use tokio::net::TcpStream;
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
    fastwebsockets::handshake::client(&SpawnExecutor, req, stream).await?;
  Ok(ws)
}

fn main() {}
