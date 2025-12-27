// Test for fragmented text frames with partial UTF-8 characters
// https://github.com/denoland/fastwebsockets/issues/122

use tokio::io::AsyncWriteExt;
use tokio::io::DuplexStream;

use fastwebsockets::FragmentCollector;
use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use fastwebsockets::Role;
use fastwebsockets::WebSocket;

#[tokio::test]
async fn test_fragmented_text_with_partial_utf8() {
  let (client, server) = tokio::io::duplex(1024);

  let server_task = tokio::spawn(async move {
    handle_server(server).await.unwrap();
  });
  let client_task = tokio::spawn(async move {
    handle_client(client).await.unwrap();
  });

  server_task.await.unwrap();
  client_task.await.unwrap();
}

async fn handle_server(
  stream: DuplexStream,
) -> Result<(), Box<dyn std::error::Error>> {
  let ws = WebSocket::after_handshake(stream, Role::Server);
  let mut ws = FragmentCollector::new(ws);

  let frame = ws.read_frame().await?;
  assert_eq!(frame.opcode, OpCode::Text);
  assert_eq!(frame.fin, true);
  let text = std::str::from_utf8(&frame.payload)?;
  assert_eq!(text, "Hello 😀!");

  Ok(())
}

async fn handle_client(
  mut stream: DuplexStream,
) -> Result<(), Box<dyn std::error::Error>> {
  // "Hello 😀!" where 😀 is U+1F600 (4 bytes in UTF-8: F0 9F 98 80)

  // Frame 1: "Hello " + first 2 bytes of emoji (fin=false)
  let mut frame1_payload = b"Hello ".to_vec();
  frame1_payload.extend_from_slice(&[0xF0, 0x9F]); // First 2 bytes of 😀
  let frame1 = create_raw_frame(false, OpCode::Text, &frame1_payload);
  stream.write_all(&frame1).await?;

  // Frame 2: last 2 bytes of emoji + "!" (fin=true, continuation)
  let mut frame2_payload = vec![0x98, 0x80]; // Last 2 bytes of 😀
  frame2_payload.extend_from_slice(b"!");
  let frame2 = create_raw_frame(true, OpCode::Continuation, &frame2_payload);
  stream.write_all(&frame2).await?;

  Ok(())
}

fn create_raw_frame(fin: bool, opcode: OpCode, payload: &[u8]) -> Vec<u8> {
  let mut frame = Vec::new();

  // First byte: FIN + opcode
  let first_byte = if fin { 0x80 } else { 0x00 } | (opcode as u8);
  frame.push(first_byte);
  // Second byte: MASK bit (0 for server->client) + payload length
  let len = payload.len();
  if len < 126 {
    frame.push(len as u8);
  } else if len < 65536 {
    frame.push(126);
    frame.extend_from_slice(&(len as u16).to_be_bytes());
  } else {
    frame.push(127);
    frame.extend_from_slice(&(len as u64).to_be_bytes());
  }
  frame.extend_from_slice(payload);

  frame
}

#[tokio::test]
async fn test_low_level_fragmented_text_with_partial_utf8() {
  // Test that the low-level WebSocket API doesn't validate UTF-8 on individual frames
  let (client, server) = tokio::io::duplex(1024);

  let server_task = tokio::spawn(async move {
    handle_server_low_level(server).await.unwrap();
  });

  let client_task = tokio::spawn(async move {
    handle_client(client).await.unwrap();
  });

  server_task.await.unwrap();
  client_task.await.unwrap();
}

async fn handle_server_low_level(
  stream: DuplexStream,
) -> Result<(), Box<dyn std::error::Error>> {
  let mut ws = WebSocket::after_handshake(stream, Role::Server);

  // should succeed even though it contains partial UTF-8
  let frame1 = ws.read_frame().await?;
  assert_eq!(frame1.opcode, OpCode::Text);
  assert_eq!(frame1.fin, false);

  // should succeed even though it starts with partial UTF-8
  let frame2 = ws.read_frame().await?;
  assert_eq!(frame2.opcode, OpCode::Continuation);
  assert_eq!(frame2.fin, true);

  // When combined, they should form valid UTF-8
  let mut combined = frame1.payload.to_vec();
  combined.extend_from_slice(&frame2.payload);
  let text = std::str::from_utf8(&combined)?;
  assert_eq!(text, "Hello 😀!");

  Ok(())
}
