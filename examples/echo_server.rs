use base64;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha1::{Digest, Sha1};
use sockdeez::{Frame, OpCode, WebSocket};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

async fn handle_client(
  mut socket: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  handshake(&mut socket).await?;

  let mut ws = WebSocket::after_handshake(socket);
  ws.set_writev(false);
  ws.set_auto_close(true);
  ws.set_auto_pong(true);

  loop {
    let frame = ws.read_frame().await?;

    match frame {
      OpCode::Close => break,
      OpCode::Text | OpCode::Binary => {
        let frame = Frame::new(true, frame.opcode, None, frame.payload);
        ws.write_frame(frame).await?;
      }
    }
  }

  Ok(())
}

async fn handshake(
  socket: &mut TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let mut reader = BufReader::new(&mut socket);
  let mut headers = Vec::new();
  loop {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    if line == "\r\n" {
      break;
    }
    headers.push(line);
  }

  let key = extract_key(headers)?;
  let response = generate_response(&key);
  socket.write_all(response.as_bytes()).await?;
  Ok(())
}

fn extract_key(
  request: Vec<String>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
  let key = request
    .iter()
    .filter_map(|line| {
      if line.starts_with("Sec-WebSocket-Key:") {
        Some(line.trim().split(":").nth(1).unwrap().trim())
      } else {
        None
      }
    })
    .next()
    .ok_or("Invalid request: missing Sec-WebSocket-Key header")?
    .to_owned();
  Ok(key)
}

fn generate_response(key: &str) -> String {
  let mut sha1 = Sha1::new();
  sha1.update(key.as_bytes());
  sha1.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"); // magic string
  let result = sha1.finalize();
  let encoded = STANDARD.encode(&result[..]);
  let response = format!(
    "HTTP/1.1 101 Switching Protocols\r\n\
                             Upgrade: websocket\r\n\
                             Connection: Upgrade\r\n\
                             Sec-WebSocket-Accept: {}\r\n\r\n",
    encoded
  );
  response
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let listener = TcpListener::bind("127.0.0.1:8080").await?;
  println!("Server started, listening on {}", "127.0.0.1:8080");
  loop {
    let (socket, _) = listener.accept().await?;
    println!("Client connected");
    tokio::spawn(async move {
      if let Err(e) = handle_client(socket).await {
        println!("An error occurred: {:?}", e);
      }
    });
  }
}
