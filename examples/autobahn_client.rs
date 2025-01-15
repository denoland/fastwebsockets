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

use std::future::Future;

use anyhow::Result;
use bytes::Bytes;
use fastwebsockets::FragmentCollector;
use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use http_body_util::Empty;
use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::Request;
use monoio::io::IntoPollIo;
use monoio::net::TcpStream;

struct SpawnExecutor;

impl<Fut> hyper::rt::Executor<Fut> for SpawnExecutor
where
  Fut: Future + Send + 'static,
  Fut::Output: Send + 'static,
{
  fn execute(&self, fut: Fut) {
    monoio::spawn(fut);
  }
}

async fn connect(path: &str) -> Result<FragmentCollector<hyper_util::rt::tokio::TokioIo<hyper::upgrade::Upgraded>>> {
  let stream = TcpStream::connect("localhost:9001").await?;
  let stream = HyperConnection(stream.into_poll_io()?);

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
    .body(Empty::<Bytes>::new())?;

  let (ws, _) =
    fastwebsockets::handshake::client(&SpawnExecutor, req, stream).await?;
  Ok(FragmentCollector::new(ws))
}

async fn get_case_count() -> Result<u32> {
  let mut ws = connect("getCaseCount").await?;
  let msg = ws.read_frame().await?;
  ws.write_frame(Frame::close(1000, &[])).await?;
  Ok(std::str::from_utf8(&msg.payload)?.parse()?)
}

#[monoio::main]
async fn main() -> Result<()> {
  let count = get_case_count().await?;

  for case in 1..=count {
    let mut ws =
      connect(&format!("runCase?case={}&agent=fastwebsockets", case)).await?;

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
        OpCode::Text | OpCode::Binary => {
          ws.write_frame(Frame::new(true, msg.opcode, None, msg.payload))
            .await?;
        }
        OpCode::Close => {
          break;
        }
        _ => {}
      }
    }
  }

  let mut ws = connect("updateReports?agent=fastwebsockets").await?;
  ws.write_frame(Frame::close(1000, &[])).await?;

  Ok(())
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
