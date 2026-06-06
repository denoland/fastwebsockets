// Copyright 2023-2026 Divy Srivastava <dj.srivastava23@gmail.com>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::Result;
use fastwebsockets::FragmentCollector;
use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use fastwebsockets::Role;
use fastwebsockets::WebSocket;
use tokio::io::AsyncWriteExt;
use tokio::io::DuplexStream;

async fn write_masked_frame(
  stream: &mut DuplexStream,
  fin: bool,
  opcode: OpCode,
  mask: [u8; 4],
  payload: &[u8],
) -> Result<()> {
  let mut masked = payload.to_vec();
  fastwebsockets::unmask(&mut masked, mask);

  let mut frame = Frame::new(fin, opcode, Some(mask), masked.into());
  let mut buf = Vec::new();
  stream.write_all(frame.write(&mut buf)).await?;
  Ok(())
}

#[tokio::test]
async fn unfragmented_masked_text_can_be_manually_unmasked() -> Result<()> {
  let (mut client, server) = tokio::io::duplex(1024);
  let mut ws = WebSocket::after_handshake(server, Role::Server);
  ws.set_auto_apply_mask(false);
  let mut ws = FragmentCollector::new(ws);

  write_masked_frame(
    &mut client,
    true,
    OpCode::Text,
    [0xff, 0xff, 0xff, 0xff],
    b"hello",
  )
  .await?;

  let mut frame = ws.read_frame().await?;
  assert_eq!(frame.opcode, OpCode::Text);
  assert_ne!(&frame.payload[..], b"hello");

  frame.unmask();
  assert_eq!(&frame.payload[..], b"hello");

  Ok(())
}

#[tokio::test]
async fn fragmented_masked_text_is_unmasked_before_validation() -> Result<()> {
  let (mut client, server) = tokio::io::duplex(1024);
  let mut ws = WebSocket::after_handshake(server, Role::Server);
  ws.set_auto_apply_mask(false);
  let mut ws = FragmentCollector::new(ws);

  write_masked_frame(
    &mut client,
    false,
    OpCode::Text,
    [0xff, 0xff, 0xff, 0xff],
    b"hello ",
  )
  .await?;
  write_masked_frame(
    &mut client,
    true,
    OpCode::Continuation,
    [0x80, 0x80, 0x80, 0x80],
    b"world",
  )
  .await?;

  let mut frame = ws.read_frame().await?;
  assert_eq!(frame.opcode, OpCode::Text);
  assert_eq!(&frame.payload[..], b"hello world");

  frame.unmask();
  assert_eq!(&frame.payload[..], b"hello world");

  Ok(())
}

#[tokio::test]
async fn fragmented_masked_binary_is_unmasked() -> Result<()> {
  let (mut client, server) = tokio::io::duplex(1024);
  let mut ws = WebSocket::after_handshake(server, Role::Server);
  ws.set_auto_apply_mask(false);
  let mut ws = FragmentCollector::new(ws);

  write_masked_frame(
    &mut client,
    false,
    OpCode::Binary,
    [0x7f, 0x7f, 0x7f, 0x7f],
    &[0, 1, 2, 3],
  )
  .await?;
  write_masked_frame(
    &mut client,
    true,
    OpCode::Continuation,
    [0x55, 0x55, 0x55, 0x55],
    &[4, 5, 6, 7],
  )
  .await?;

  let frame = ws.read_frame().await?;
  assert_eq!(frame.opcode, OpCode::Binary);
  assert_eq!(&frame.payload[..], &[0, 1, 2, 3, 4, 5, 6, 7]);

  Ok(())
}
