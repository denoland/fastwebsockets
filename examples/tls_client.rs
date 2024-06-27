use std::future::Future;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use fastwebsockets::FragmentCollector;
use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use http_body_util::Empty;
use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::upgrade::Upgraded;
use hyper::Request;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::OwnedTrustAnchor;
use tokio_rustls::TlsConnector;

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

fn tls_connector() -> Result<TlsConnector> {
  let mut root_store = tokio_rustls::rustls::RootCertStore::empty();

  root_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(
    |ta| {
      OwnedTrustAnchor::from_subject_spki_name_constraints(
        ta.subject,
        ta.spki,
        ta.name_constraints,
      )
    },
  ));

  let config = ClientConfig::builder()
    .with_safe_defaults()
    .with_root_certificates(root_store)
    .with_no_client_auth();

  Ok(TlsConnector::from(Arc::new(config)))
}

async fn connect(domain: &str) -> Result<FragmentCollector<TokioIo<Upgraded>>> {
  let mut addr = String::from(domain);
  addr.push_str(":9443"); // Port number for binance stream

  let tcp_stream = TcpStream::connect(&addr).await?;
  let tls_connector = tls_connector().unwrap();
  let domain =
    tokio_rustls::rustls::ServerName::try_from(domain).map_err(|_| {
      std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid dnsname")
    })?;

  let tls_stream = tls_connector.connect(domain, tcp_stream).await?;

  let req = Request::builder()
    .method("GET")
    .uri(format!("wss://{}/ws/btcusdt@bookTicker", &addr)) //stream we want to subscribe to
    .header("Host", &addr)
    .header(UPGRADE, "websocket")
    .header(CONNECTION, "upgrade")
    .header(
      "Sec-WebSocket-Key",
      fastwebsockets::handshake::generate_key(),
    )
    .header("Sec-WebSocket-Version", "13")
    .body(Empty::<Bytes>::new())?;

  let (ws, _) =
    fastwebsockets::handshake::client(&SpawnExecutor, req, tls_stream).await?;
  Ok(FragmentCollector::new(ws))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
  let domain = "data-stream.binance.com";
  let mut ws = connect(domain).await?;

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
