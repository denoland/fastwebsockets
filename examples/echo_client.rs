use bytes::Bytes;
use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use fastwebsockets::WebSocketError;
use fastwebsockets::{self};
use http_body_util::Empty;
use hyper::Request;
use hyper::Uri;
use monoio::io::IntoPollIo;
use monoio::net::TcpStream;
use rand::Rng;
use std::future::Future;

async fn handle_websocket_upgrade(uri: Uri) -> Result<(), WebSocketError> {
  let stream = TcpStream::connect("127.0.0.1:8080").await?;
  let conn = HyperConnection(stream.into_poll_io()?);

  let sec_websocket_key = generate_sec_websocket_key();
  let req = Request::builder()
    .method("GET")
    .uri(uri)
    .header("Upgrade", "websocket")
    .header("Connection", "Upgrade")
    .header("Sec-WebSocket-Key", sec_websocket_key)
    .header("Sec-WebSocket-Version", "13") // WebSocket 版本
    .body(Empty::<Bytes>::new())
    .expect("Failed to build request");
  let (mut ws, _) =
    fastwebsockets::handshake::client(&HyperExecutor, req, conn).await?;

  for _ in 0..1000 {
    let frame = fastwebsockets::Frame::new(
      true, // fin
      OpCode::Text,
      None, // mask
      fastwebsockets::Payload::from("hello".as_bytes().to_vec()),
    );
    ws.write_frame(frame).await?;
    let msg = match ws.read_frame().await {
      Ok(msg) => msg,
      Err(e) => {
        println!("Error: {}", e);
        ws.write_frame(Frame::close_raw(vec![].into())).await?;
        break;
      }
    };

    match msg.opcode {
      OpCode::Text => {
        let payload =
          String::from_utf8(msg.payload.to_vec()).expect("Invalid UTF-8 data");
        // Normally deserialise from json here, print just to show it works
        println!("{:?}", payload);
      }
      OpCode::Close => {
        break;
      }
      _ => {}
    }
  }
  Ok(())
}

#[monoio::main]
async fn main() -> Result<(), WebSocketError> {
  let uri: Uri = "ws://127.0.0.1:8080".parse().unwrap();
  handle_websocket_upgrade(uri).await
}

#[allow(deprecated)]
fn generate_sec_websocket_key() -> String {
  let mut rng = rand::thread_rng();
  let random_bytes: Vec<u8> = (0..16).map(|_| rng.gen()).collect();
  base64::encode(random_bytes)
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