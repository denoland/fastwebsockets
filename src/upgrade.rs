// Port of hyper_tunstenite for fastwebsockets.
// https://github.com/de-vri-es/hyper-tungstenite-rs
//
// Copyright 2021, Maarten de Vries maarten@de-vri.es
// BSD 2-Clause "Simplified" License
//
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

use base64;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::Request;
use hyper::Response;
use hyper_util::rt::TokioIo;
use pin_project::pin_project;
use sha1::Digest;
use sha1::Sha1;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use crate::Role;
use crate::WebSocket;
use crate::WebSocketError;

fn sec_websocket_protocol(key: &[u8]) -> String {
  let mut sha1 = Sha1::new();
  sha1.update(key);
  sha1.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"); // magic string
  let result = sha1.finalize();
  STANDARD.encode(&result[..])
}

type Error = WebSocketError;

pub struct IncomingUpgrade {
  key: String,
  on_upgrade: hyper::upgrade::OnUpgrade,
}

impl IncomingUpgrade {
  pub fn upgrade(self) -> Result<(Response<Empty<Bytes>>, UpgradeFut), Error> {
    let response = Response::builder()
      .status(hyper::StatusCode::SWITCHING_PROTOCOLS)
      .header(hyper::header::CONNECTION, "upgrade")
      .header(hyper::header::UPGRADE, "websocket")
      .header("Sec-WebSocket-Accept", self.key)
      .body(Empty::new())
      .expect("bug: failed to build response");

    let stream = UpgradeFut {
      inner: self.on_upgrade,
    };

    Ok((response, stream))
  }
}

/// A future that resolves to a websocket stream when the associated HTTP upgrade completes.
#[pin_project]
#[derive(Debug)]
pub struct UpgradeFut {
  #[pin]
  inner: hyper::upgrade::OnUpgrade,
}

/// Try to upgrade a received `hyper::Request` to a websocket connection.
///
/// The function returns a HTTP response and a future that resolves to the websocket stream.
/// The response body *MUST* be sent to the client before the future can be resolved.
///
/// This functions checks `Sec-WebSocket-Key` and `Sec-WebSocket-Version` headers.
/// It does not inspect the `Origin`, `Sec-WebSocket-Protocol` or `Sec-WebSocket-Extensions` headers.
/// You can inspect the headers manually before calling this function,
/// and modify the response headers appropriately.
///
/// This function also does not look at the `Connection` or `Upgrade` headers.
/// To check if a request is a websocket upgrade request, you can use [`is_upgrade_request`].
/// Alternatively you can inspect the `Connection` and `Upgrade` headers manually.
///
pub fn upgrade<B>(
  mut request: impl std::borrow::BorrowMut<Request<B>>,
) -> Result<(Response<Empty<Bytes>>, UpgradeFut), Error> {
  let request = request.borrow_mut();

  let key = request
    .headers()
    .get("Sec-WebSocket-Key")
    .ok_or(WebSocketError::MissingSecWebSocketKey)?;
  if request
    .headers()
    .get("Sec-WebSocket-Version")
    .map(|v| v.as_bytes())
    != Some(b"13")
  {
    return Err(WebSocketError::InvalidSecWebsocketVersion);
  }

  let response = Response::builder()
    .status(hyper::StatusCode::SWITCHING_PROTOCOLS)
    .header(hyper::header::CONNECTION, "upgrade")
    .header(hyper::header::UPGRADE, "websocket")
    .header(
      "Sec-WebSocket-Accept",
      &sec_websocket_protocol(key.as_bytes()),
    )
    .body(Empty::new())
    .expect("bug: failed to build response");

  let stream = UpgradeFut {
    inner: hyper::upgrade::on(request),
  };

  Ok((response, stream))
}

/// Check if a request is a websocket upgrade request.
///
/// If the `Upgrade` header lists multiple protocols,
/// this function returns true if of them are `"websocket"`,
/// If the server supports multiple upgrade protocols,
/// it would be more appropriate to try each listed protocol in order.
pub fn is_upgrade_request<B>(request: &hyper::Request<B>) -> bool {
  header_contains_value(request.headers(), hyper::header::CONNECTION, "Upgrade")
    && header_contains_value(
      request.headers(),
      hyper::header::UPGRADE,
      "websocket",
    )
}

/// Check if there is a header of the given name containing the wanted value.
fn header_contains_value(
  headers: &hyper::HeaderMap,
  header: impl hyper::header::AsHeaderName,
  value: impl AsRef<[u8]>,
) -> bool {
  let value = value.as_ref();
  for header in headers.get_all(header) {
    if header
      .as_bytes()
      .split(|&c| c == b',')
      .any(|x| trim(x).eq_ignore_ascii_case(value))
    {
      return true;
    }
  }
  false
}

fn trim(data: &[u8]) -> &[u8] {
  trim_end(trim_start(data))
}

fn trim_start(data: &[u8]) -> &[u8] {
  if let Some(start) = data.iter().position(|x| !x.is_ascii_whitespace()) {
    &data[start..]
  } else {
    b""
  }
}

fn trim_end(data: &[u8]) -> &[u8] {
  if let Some(last) = data.iter().rposition(|x| !x.is_ascii_whitespace()) {
    &data[..last + 1]
  } else {
    b""
  }
}

impl std::future::Future for UpgradeFut {
  type Output = Result<WebSocket<TokioIo<hyper::upgrade::Upgraded>>, Error>;

  fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
    let this = self.project();
    let upgraded = match this.inner.poll(cx) {
      Poll::Pending => return Poll::Pending,
      Poll::Ready(x) => x,
    };
    Poll::Ready(Ok(WebSocket::after_handshake(
      TokioIo::new(upgraded?),
      Role::Server,
    )))
  }
}