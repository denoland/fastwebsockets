#[tokio::test]
async fn simple() -> Result<(), fastwebsockets::WebSocketError> {
  let url = "wss://fstream.binance.com/ws/Estn2SL73HZfTdKCetdGmWPKrT0gpdl0JUnV9QgkZnUZ2eMwtTbcN42zTOOtxAe9";
  let mut ws = fastwebsockets::handshake::connect(url).await?;
  let json = r#"
  {
    "method": "REQUEST",
    "params":
    [
    "Estn2SL73HZfTdKCetdGmWPKrT0gpdl0JUnV9QgkZnUZ2eMwtTbcN42zTOOtxAe9@account",
    "Estn2SL73HZfTdKCetdGmWPKrT0gpdl0JUnV9QgkZnUZ2eMwtTbcN42zTOOtxAe9@balance"
    ],
    "id": 777
  }"#;
  ws.write_frame(fastwebsockets::Frame::text(
    fastwebsockets::Payload::Borrowed(json.as_bytes()),
  ))
  .await?;
  let result = ws.read_frame().await?;
  println!(
    "{}",
    String::from_utf8_lossy(result.payload.to_vec().as_slice())
  );
  Ok(())
}
