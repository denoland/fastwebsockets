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

use std::error::Error;
use std::future::Future;

use crate::Role;
use crate::WebSocket;

pub async fn client<S, E>(
  executor: &E,
  request: Request<Body>,
  socket: S,
) -> Result<(WebSocket<Upgraded>, Response<Body>), Box<dyn Error + Send + Sync>>
where
  S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
  E: hyper::rt::Executor<Box<dyn Future<Output = ()> + Send + Unpin>> + 'static,
{
  let (mut sender, conn) = hyper::client::conn::handshake(socket).await?;
  let fut = Box::pin(async move {
    if let Err(e) = conn.await {
      eprintln!("Error polling connection: {}", e);
    }
  });
  executor.execute(Box::new(fut));

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
fn verify(
  response: &Response<Body>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
  if response.status() != StatusCode::SWITCHING_PROTOCOLS {
    return Err("Invalid status code".into());
  }

  let headers = response.headers();

  if !headers
    .get("Upgrade")
    .and_then(|h| h.to_str().ok())
    .map(|h| h.eq_ignore_ascii_case("websocket"))
    .unwrap_or(false)
  {
    return Err("Invalid Upgrade header".into());
  }

  if !headers
    .get("Connection")
    .and_then(|h| h.to_str().ok())
    .map(|h| h.eq_ignore_ascii_case("Upgrade"))
    .unwrap_or(false)
  {
    return Err("Invalid Connection header".into());
  }

  Ok(())
}
