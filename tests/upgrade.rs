// https://github.com/de-vri-es/hyper-tungstenite-rs/tree/main/tests

use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::server::Server;
use hyper::service::make_service_fn;
use hyper::service::service_fn;
use hyper::Body;
use hyper::Request;
use hyper::Response;
use std::future::Future;
use std::net::Ipv6Addr;
use tokio::net::TcpStream;

use assert2::assert;
use assert2::let_assert;

struct TestExecutor;

impl<Fut> hyper::rt::Executor<Fut> for TestExecutor
where
  Fut: Future + Send + 'static,
  Fut::Output: Send + 'static,
{
  fn execute(&self, fut: Fut) {
    tokio::spawn(fut);
  }
}

#[tokio::test]
async fn hyper() {
  // Bind a TCP listener to an ephemeral port.
  let_assert!(
    Ok(listener) = std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0u16))
  );
  let_assert!(Ok(bind_addr) = listener.local_addr());
  let_assert!(Ok(server) = Server::from_tcp(listener));

  // Spawn the server in a task.
  tokio::spawn(async move {
    let service = make_service_fn(|_conn| async {
      Ok::<_, hyper::Error>(service_fn(upgrade_websocket))
    });
    let_assert!(Ok(()) = server.http1_only(true).serve(service).await);
  });

  // Try to create a websocket connection with the server.
  let_assert!(Ok(stream) = TcpStream::connect(bind_addr).await);
  let_assert!(
    Ok(req) = Request::builder()
      .method("GET")
      .uri("ws://localhost/foo")
      .header("Host", "localhost")
      .header(UPGRADE, "websocket")
      .header(CONNECTION, "upgrade")
      .header(
        "Sec-WebSocket-Key",
        fastwebsockets::handshake::generate_key(),
      )
      .header("Sec-WebSocket-Version", "13")
      .body(Body::empty())
  );
  let_assert!(Ok((mut stream, _response)) = fastwebsockets::handshake::client(&TestExecutor, req, stream).await);

  let_assert!(Ok(message) = stream.read_frame().await);
  assert!(message.opcode == fastwebsockets::OpCode::Text);
  assert!(message.payload == b"Hello!");

  let_assert!(
    Ok(()) = stream
      .write_frame(fastwebsockets::Frame::text(b"Goodbye!".to_vec().into()))
      .await
  );
  let_assert!(Ok(close_frame) = stream.read_frame().await);
  assert!(close_frame.opcode == fastwebsockets::OpCode::Close);
}

async fn upgrade_websocket(
  mut request: Request<Body>,
) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
  assert!(fastwebsockets::upgrade::is_upgrade_request(&request) == true);

  let (response, stream) = fastwebsockets::upgrade::upgrade(&mut request)?;
  tokio::spawn(async move {
    let_assert!(Ok(mut stream) = stream.await);
    assert!(let Ok(()) = stream.write_frame(fastwebsockets::Frame::text(b"Hello!".to_vec().into())).await);
    let_assert!(Ok(reply) = stream.read_frame().await);
    assert!(reply.opcode == fastwebsockets::OpCode::Text);
    assert!(reply.payload == b"Goodbye!");

    assert!(let Ok(()) = stream.write_frame(fastwebsockets::Frame::close_raw(vec![].into())).await);
  });

  Ok(response)
}
