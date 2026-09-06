use fastwebsockets::FragmentCollector;
#[cfg(feature = "unstable-split")]
use fastwebsockets::FragmentCollectorRead;
use fastwebsockets::Frame;
use fastwebsockets::OpCode;
use fastwebsockets::Role;
use fastwebsockets::WebSocket;
use fastwebsockets::WebSocketError;
use tokio::io::AsyncWriteExt;

fn encoded_frames(mut frames: Vec<Frame<'static>>) -> Vec<u8> {
  let mut out = Vec::new();
  let mut scratch = Vec::new();

  for frame in &mut frames {
    out.extend_from_slice(frame.write(&mut scratch));
  }

  out
}

fn assert_frame_too_large<T>(result: Result<T, WebSocketError>) {
  assert!(matches!(result, Err(WebSocketError::FrameTooLarge)));
}

#[tokio::test]
async fn fragment_collector_rejects_aggregate_binary_over_limit() {
  let (mut peer, socket) = tokio::io::duplex(1024);
  let mut ws = WebSocket::after_handshake(socket, Role::Client);
  ws.set_max_message_size(9);
  let mut ws = FragmentCollector::new(ws);

  let frames = encoded_frames(vec![
    Frame::new(false, OpCode::Binary, None, b"12345".to_vec().into()),
    Frame::new(true, OpCode::Continuation, None, b"67890".to_vec().into()),
  ]);
  peer.write_all(&frames).await.unwrap();

  assert_frame_too_large(ws.read_frame().await);
}

#[tokio::test]
async fn fragment_collector_rejects_aggregate_text_over_limit() {
  let (mut peer, socket) = tokio::io::duplex(1024);
  let mut ws = WebSocket::after_handshake(socket, Role::Client);
  ws.set_max_message_size(9);
  let mut ws = FragmentCollector::new(ws);

  let frames = encoded_frames(vec![
    Frame::new(false, OpCode::Text, None, b"hello".to_vec().into()),
    Frame::new(true, OpCode::Continuation, None, b"world".to_vec().into()),
  ]);
  peer.write_all(&frames).await.unwrap();

  assert_frame_too_large(ws.read_frame().await);
}

#[cfg(feature = "unstable-split")]
#[tokio::test]
async fn split_fragment_collector_rejects_aggregate_binary_over_limit() {
  let (mut peer, socket) = tokio::io::duplex(1024);
  let (read, _write) = tokio::io::split(socket);
  let (mut ws_read, _ws_write) = fastwebsockets::after_handshake_split(
    read,
    tokio::io::sink(),
    Role::Client,
  );
  ws_read.set_max_message_size(9);
  let mut ws = FragmentCollectorRead::new(ws_read);

  let frames = encoded_frames(vec![
    Frame::new(false, OpCode::Binary, None, b"12345".to_vec().into()),
    Frame::new(true, OpCode::Continuation, None, b"67890".to_vec().into()),
  ]);
  peer.write_all(&frames).await.unwrap();

  assert_frame_too_large(
    ws.read_frame(&mut |_| async { Ok::<(), std::io::Error>(()) })
      .await,
  );
}
