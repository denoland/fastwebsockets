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

use fastwebsockets::FragmentCollector;
use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::upgrade::Upgraded;
use hyper::Body;
use hyper::Request;
use tokio::net::TcpStream;

type Result<T> =
  std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

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

async fn connect(path: &str) -> Result<FragmentCollector<Upgraded>> {
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
  Ok(FragmentCollector::new(ws))
}

async fn get_case_count() -> Result<u32> {
  let mut ws = connect("getCaseCount").await?;
  let msg = ws.read_frame().await?;
  ws.write_frame(Frame::close(1000, &[])).await?;
  Ok(std::str::from_utf8(&msg.payload)?.parse()?)
}

#[tokio::main(flavor = "current_thread")]
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
