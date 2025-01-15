use anyhow::Result;
use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use fastwebsockets::WebSocketError;
use fastwebsockets::{self};
use hyper::Request;
use hyper::Uri;
use monoio::net::TcpStream;
use std::future::Future;
use std::sync::Arc;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::OwnedTrustAnchor;
use tokio_rustls::TlsConnector;
use monoio::io::IntoPollIo;
use bytes::Bytes;
use http_body_util::Empty;

#[allow(deprecated)]
fn tls_connector() -> Result<TlsConnector> {
  let mut root_store = tokio_rustls::rustls::RootCertStore::empty();

  root_store.add_server_trust_anchors(
    webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
      OwnedTrustAnchor::from_subject_spki_name_constraints(
        ta.subject,
        ta.spki,
        ta.name_constraints,
      )
    }),
  );

  let config = ClientConfig::builder()
    .with_safe_defaults()
    .with_root_certificates(root_store)
    .with_no_client_auth();

  Ok(TlsConnector::from(Arc::new(config)))
}

async fn handle_websocket_upgrade(
  uri: Uri,
  port: u16,
) -> Result<(), WebSocketError> {
  // 1. 创建HTTP客户端
  let host = uri.host().expect("uri has no host");
  let port = uri.port_u16().unwrap_or(port);
  let addr = format!("{}:{}", host, port);
  let stream = TcpStream::connect(&addr).await?;
  let tcp_stream = HyperConnection(stream.into_poll_io()?);
  println!("Connected to: {:?}", addr);
  let domain =
    tokio_rustls::rustls::ServerName::try_from(uri.to_string().as_str())
      .map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid dnsname")
      })?;

  println!("Domin found: {:?}", domain);
  let tls_connector = tls_connector().unwrap();
  let tls_stream = tls_connector.connect(domain, tcp_stream).await.unwrap();

  let req = Request::builder()
    .method("GET")
    .uri(format!("wss://{}/ws/btcusdt@bookTicker", &addr))
    .header("Host", &addr)
    .header("Upgrade", "websocket")
    .header("Connection", "Upgrade")
    .header(
      "Sec-WebSocket-Key",
      fastwebsockets::handshake::generate_key(),
    )
    .header("Sec-WebSocket-Version", "13") // WebSocket 版本
    .body(Empty::<Bytes>::new())
    .expect("Failed to build request");

  let (mut ws, _) =
    fastwebsockets::handshake::client(&HyperExecutor, req, tls_stream).await?;
  println!("WebSocket handshake succeeded");
  loop {
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
async fn main() {
  // !!!!! do not use proxychains or other proxy tools, the tcp steam connect may be failed !!!!
  let uri: Uri = "data-stream.binance.com".parse::<hyper::Uri>().unwrap();
  let port = 9443;
  handle_websocket_upgrade(uri, port).await.unwrap();
}

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


unsafe impl Send for HyperConnection {}
