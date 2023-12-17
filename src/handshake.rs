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

use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::upgrade::Upgraded;
use hyper::Request;
use hyper::Response;
use hyper::StatusCode;
use hyper::Uri;
use std::sync::Arc;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::OwnedTrustAnchor;
use tokio_rustls::TlsConnector;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use hyper_util::rt::TokioIo;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;

use std::future::Future;
use std::pin::Pin;

use crate::Role;
use crate::WebSocket;
use crate::WebSocketError;

/// Perform the client handshake.
///
/// This function is used to perform the client handshake. It takes a hyper
/// executor, a `hyper::Request` and a stream.
///
/// # Example
///
/// ```
/// use fastwebsockets::handshake;
/// use fastwebsockets::WebSocket;
/// use hyper::{Request, body::Bytes, upgrade::Upgraded, header::{UPGRADE, CONNECTION}};
/// use hyper_util::rt::TokioIo;
/// use http_body_util::Empty;
/// use tokio::net::TcpStream;
/// use std::future::Future;
/// use anyhow::Result;
///
/// async fn connect() -> Result<WebSocket<TokioIo<Upgraded>>> {
///   let stream = TcpStream::connect("localhost:9001").await?;
///
///   let req = Request::builder()
///     .method("GET")
///     .uri("http://localhost:9001/")
///     .header("Host", "localhost:9001")
///     .header(UPGRADE, "websocket")
///     .header(CONNECTION, "upgrade")
///     .header(
///       "Sec-WebSocket-Key",
///       fastwebsockets::handshake::generate_key(),
///     )
///     .header("Sec-WebSocket-Version", "13")
///     .body(Empty::<Bytes>::new())?;
///
///   let (ws, _) = handshake::client(&SpawnExecutor, req, stream).await?;
///   Ok(ws)
/// }
///
/// // Tie hyper's executor to tokio runtime
/// struct SpawnExecutor;
///
/// impl<Fut> hyper::rt::Executor<Fut> for SpawnExecutor
/// where
///   Fut: Future + Send + 'static,
///   Fut::Output: Send + 'static,
/// {
///   fn execute(&self, fut: Fut) {
///     tokio::task::spawn(fut);
///   }
/// }
/// ```
pub async fn client<S, E, B>(
  executor: &E,
  request: Request<B>,
  socket: S,
) -> Result<(WebSocket<TokioIo<Upgraded>>, Response<Incoming>), WebSocketError>
where
  S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
  E: hyper::rt::Executor<Pin<Box<dyn Future<Output = ()> + Send>>>,
  B: hyper::body::Body + 'static + Send,
  B::Data: Send,
  B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
  let (mut sender, conn) =
    hyper::client::conn::http1::handshake(TokioIo::new(socket)).await?;
  let fut = Box::pin(async move {
    if let Err(e) = conn.with_upgrades().await {
      eprintln!("Error polling connection: {}", e);
    }
  });
  executor.execute(fut);

  let mut response = sender.send_request(request).await?;
  verify(&response)?;

  match hyper::upgrade::on(&mut response).await {
    Ok(upgraded) => Ok((
      WebSocket::after_handshake(TokioIo::new(upgraded), Role::Client),
      response,
    )),
    Err(e) => Err(e.into()),
  }
}

/// Generate a random key for the `Sec-WebSocket-Key` header.
pub fn generate_key() -> String {
  // a base64-encoded (see Section 4 of [RFC4648]) value that,
  // when decoded, is 16 bytes in length (RFC 6455)
  let r: [u8; 16] = rand::random();
  STANDARD.encode(&r)
}

// https://github.com/snapview/tungstenite-rs/blob/314feea3055a93e585882fb769854a912a7e6dae/src/handshake/client.rs#L189
fn verify(response: &Response<Incoming>) -> Result<(), WebSocketError> {
  if response.status() != StatusCode::SWITCHING_PROTOCOLS {
    return Err(WebSocketError::InvalidStatusCode(
      response.status().as_u16(),
    ));
  }

  let headers = response.headers();

  if !headers
    .get("Upgrade")
    .and_then(|h| h.to_str().ok())
    .map(|h| h.eq_ignore_ascii_case("websocket"))
    .unwrap_or(false)
  {
    return Err(WebSocketError::InvalidUpgradeHeader);
  }

  if !headers
    .get("Connection")
    .and_then(|h| h.to_str().ok())
    .map(|h| h.eq_ignore_ascii_case("Upgrade"))
    .unwrap_or(false)
  {
    return Err(WebSocketError::InvalidConnectionHeader);
  }

  Ok(())
}

/// Trait for converting various types into HTTP requests used for a client connection.
pub trait IntoClientRequest<B>
where
  B: hyper::body::Body + 'static + Send,
  B::Data: Send,
  B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
  /// Convert into a `Request` that can be used for a client connection.
  fn into_client_request(self) -> Result<Request<B>, WebSocketError>;
}

impl<'a> IntoClientRequest<Empty<Bytes>> for &'a str {
  fn into_client_request(
    self,
  ) -> Result<Request<Empty<Bytes>>, WebSocketError> {
    self
      .parse::<Uri>()
      .map_err(|_| WebSocketError::InvalidUri)?
      .into_client_request()
  }
}

impl<'a> IntoClientRequest<Empty<Bytes>> for &'a String {
  fn into_client_request(
    self,
  ) -> Result<Request<Empty<Bytes>>, WebSocketError> {
    <&str as IntoClientRequest<Empty<Bytes>>>::into_client_request(self)
  }
}

impl IntoClientRequest<Empty<Bytes>> for String {
  fn into_client_request(
    self,
  ) -> Result<Request<Empty<Bytes>>, WebSocketError> {
    <&str as IntoClientRequest<Empty<Bytes>>>::into_client_request(&self)
  }
}

impl<'a> IntoClientRequest<Empty<Bytes>> for &'a Uri {
  fn into_client_request(
    self,
  ) -> Result<Request<Empty<Bytes>>, WebSocketError> {
    self.clone().into_client_request()
  }
}

impl IntoClientRequest<Empty<Bytes>> for Uri {
  fn into_client_request(
    self,
  ) -> Result<Request<Empty<Bytes>>, WebSocketError> {
    let authority =
      self.authority().ok_or(WebSocketError::NoHostName)?.as_str();
    let host = authority
      .find('@')
      .map(|idx| authority.split_at(idx + 1).1)
      .unwrap_or_else(|| authority);

    if host.is_empty() {
      return Err(WebSocketError::EmptyHostName);
    }

    let req = Request::builder()
      .method("GET")
      .header("Host", host)
      .header("Connection", "Upgrade")
      .header("Upgrade", "websocket")
      .header("Sec-WebSocket-Version", "13")
      .header("Sec-WebSocket-Key", generate_key())
      .uri(self)
      .body(Empty::<Bytes>::new())?;

    Ok(req)
  }
}

/// Connect WebSocket.
///
/// The URL may be either ws:// or wss://.
///
/// # Example
///
/// ```
/// #[tokio::test]
/// async fn simple() -> Result<(), fastwebsockets::WebSocketError> {
///   let url = "wss://fstream.binance.com/ws/Estn2SL73HZfTdKCetdGmWPKrT0gpdl0JUnV9QgkZnUZ2eMwtTbcN42zTOOtxAe9";
///   let mut ws = fastwebsockets::handshake::connect(url).await?;
///   let json = r#"
///   {
///     "method": "REQUEST",
///     "params":
///     [
///     "Estn2SL73HZfTdKCetdGmWPKrT0gpdl0JUnV9QgkZnUZ2eMwtTbcN42zTOOtxAe9@account",
///     "Estn2SL73HZfTdKCetdGmWPKrT0gpdl0JUnV9QgkZnUZ2eMwtTbcN42zTOOtxAe9@balance"
///     ],
///     "id": 777
///   }"#;
///   ws.write_frame(fastwebsockets::Frame::text(
///     fastwebsockets::Payload::Borrowed(json.as_bytes()),
///   ))
///   .await?;
///   let result = ws.read_frame().await?;
///   println!(
///     "{}",
///     String::from_utf8_lossy(result.payload.to_vec().as_slice())
///   );
///   Ok(())
/// }
/// ```
pub async fn connect<R, B>(
  request: R,
) -> Result<WebSocket<TokioIo<Upgraded>>, WebSocketError>
where
  R: IntoClientRequest<B>,
  B: hyper::body::Body + 'static + Send,
  B::Data: Send,
  B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
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

  let request = request.into_client_request()?;
  let host = request.uri().host().ok_or(WebSocketError::NoHostName)?;
  let port = request
    .uri()
    .port_u16()
    .or_else(|| match request.uri().scheme_str() {
      Some("wss") => Some(443),
      Some("ws") => Some(80),
      _ => None,
    })
    .ok_or(WebSocketError::UnsupportedUrlScheme)?;
  let stream = tokio::net::TcpStream::connect(format!("{host}:{port}"))
    .await
    .map_err(|v| WebSocketError::IoError(v))?;

  if port == 443 {
    let domain = tokio_rustls::rustls::ServerName::try_from(host)
      .map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid dnsname")
      })?
      .to_owned();
    let tls_stream = tls_connector()?.connect(domain, stream).await?;
    return client(&SpawnExecutor, request, tls_stream)
      .await
      .map(|v| v.0);
  }

  client(&SpawnExecutor, request, stream).await.map(|v| v.0)
}

/// Constructs a TLS connector with default configuration and root certificates from webpki_roots.
///
/// This function initializes a `RootCertStore` with trust anchors from the webpki_roots crate
/// and creates a TLS connector using the tokio_rustls library.
pub fn tls_connector() -> Result<TlsConnector, WebSocketError> {
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
