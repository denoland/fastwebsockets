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

use hyper::upgrade::Upgraded;
use hyper::Body;
use hyper::Request;
use hyper::Response;
use hyper::StatusCode;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

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
/// use hyper::{Request, Body, upgrade::Upgraded, header::{UPGRADE, CONNECTION}};
/// use tokio::net::TcpStream;
/// use std::future::Future;
/// use anyhow::Result;
///
/// async fn connect() -> Result<WebSocket<Upgraded>> {
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
///     .body(Body::empty())?;
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
pub async fn client<S, E>(
  executor: &E,
  request: Request<Body>,
  socket: S,
) -> Result<(WebSocket<Upgraded>, Response<Body>), WebSocketError>
where
  S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
  E: hyper::rt::Executor<Pin<Box<dyn Future<Output = ()> + Send>>>,
{
  let (mut sender, conn) = hyper::client::conn::handshake(socket).await?;
  let fut = Box::pin(async move {
    if let Err(e) = conn.await {
      eprintln!("Error polling connection: {}", e);
    }
  });
  executor.execute(fut);

  let mut response = sender.send_request(request).await?;
  verify(&response)?;

  match hyper::upgrade::on(&mut response).await {
    Ok(upgraded) => {
      Ok((WebSocket::after_handshake(upgraded, Role::Client), response))
    }
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
fn verify(response: &Response<Body>) -> Result<(), WebSocketError> {
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
