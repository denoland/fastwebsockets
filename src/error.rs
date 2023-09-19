use thiserror::Error;

#[derive(Error, Debug)]
pub enum WebSocketError {
  #[error("Invalid fragment")]
  InvalidFragment,
  #[error("Invalid UTF-8")]
  InvalidUTF8,
  #[error("Invalid continuation frame")]
  InvalidContinuationFrame,
  #[error("Invalid status code")]
  InvalidStatusCode,
  #[error("Invalid upgrade header")]
  InvalidUpgradeHeader,
  #[error("Invalid connection header")]
  InvalidConnectionHeader,
  #[error("Connection is closed")]
  ConnectionClosed,
  #[error("Invalid close frame")]
  InvalidCloseFrame,
  #[error("Invalid close code")]
  InvalidCloseCode,
  #[error("Unexpected EOF")]
  UnexpectedEOF,
  #[error("Reserved bits are not zero")]
  ReservedBitsNotZero,
  #[error("Control frame must not be fragmented")]
  ControlFrameFragmented,
  #[error("Ping frame too large")]
  PingFrameTooLarge,
  #[error("Frame too large")]
  FrameTooLarge,
  #[error("Sec-Websocket-Version must be 13")]
  InvalidSecWebsocketVersion,
  #[error("Invalid value")]
  InvalidValue,
  #[error("Sec-WebSocket-Key header is missing")]
  MissingSecWebSocketKey,
  #[error(transparent)]
  IoError(#[from] std::io::Error),
  #[cfg(feature = "upgrade")]
  #[error(transparent)]
  HTTPError(#[from] hyper::Error),
  #[error("Failed to send frame")]
  SendError(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),
}
