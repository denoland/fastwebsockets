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

use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use fastwebsockets::WebSocket;
use futures::TryStreamExt;
use hyper::header::CONNECTION;
use hyper::upgrade::Upgraded;
use hyper::Body;
use std::io::Write;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use hyper::header::UPGRADE;
use hyper::Request;
use hyper::StatusCode;

type Result<T> =
  std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn connect(path: &str) -> Result<WebSocket<TcpStream>> {
  let mut stream = TcpStream::connect("localhost:9001").await?;

  let mut req = Vec::new();
  write!(req, "GET /{} HTTP/1.1\r\n", path).unwrap();
  write!(req, "Host: localhost:9001\r\n").unwrap();
  write!(req, "User-Agent: fastwebsockets\r\n").unwrap();
  write!(req, "Upgrade: websocket\r\n").unwrap();
  write!(req, "Connection: upgrade\r\n").unwrap();
  write!(req, "Sec-WebSocket-Key: gn/tcQDBSTmTj39Xf8bBNg==\r\n").unwrap();
  write!(req, "Sec-WebSocket-Version: 13\r\n").unwrap();
  write!(req, "\r\n").unwrap();

  stream.write_all(&req).await?;

  // Parse response
  // Peek and read. Don't read the websocket data
  let mut buf = [0; 1024];
  let n = stream.peek(&mut buf).await?;
  // Find the end of the headers and split the buffer
  let pos = buf[..n]
    .windows(4)
    .position(|window| window == b"\r\n\r\n")
    .expect("No end of headers");

  stream.read_exact(&mut buf[..pos + 4]).await?;

  Ok(WebSocket::after_handshake(stream))
}

async fn get_case_count() -> Result<u32> {
  let mut ws = connect("getCaseCount").await?;
  let msg = ws.read_frame().await?;
  ws.write_frame(Frame::close(1000, &[])).await?;
  Ok(std::str::from_utf8(&msg.payload)?.parse()?)
}

fn mask(payload: &[u8]) -> (Vec<u8>, [u8; 4]) {
  let mut mask = [1,2,3,4];
  let mut masked = Vec::new();
  for (i, byte) in payload.iter().enumerate() {
    masked.push(byte ^ mask[i % 4]);
  }
  (masked, mask)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
  let count = get_case_count().await?;
  dbg!(count);
  for case in 1..=count {
    let mut ws =
      connect(&format!("runCase?case={}&agent=fastwebsockets", case)).await?;
    dbg!(case);
    loop {
      let msg = ws.read_frame().await?;
      
      match msg.opcode {
        OpCode::Text | OpCode::Binary => {
          let (masked, mask) = mask(&msg.payload);
          ws.write_frame(Frame::new(
            true,
            msg.opcode,
            Some(mask),
            masked,
          )).await?;
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
