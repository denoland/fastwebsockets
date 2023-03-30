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
use sha1::Digest;
use sha1::Sha1;
use sockdeez::Frame;
use sockdeez::OpCode;
use sockdeez::WebSocket;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

enum Fragment {
  Text(Option<utf8::Incomplete>, Vec<u8>),
  Binary(Vec<u8>),
}

impl Fragment {
  fn take_buffer(self) -> Vec<u8> {
    match self {
      Fragment::Text(_, buffer) => buffer,
      Fragment::Binary(buffer) => buffer,
    }
  }
}

async fn handle_client(
  socket: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let socket = handshake(socket).await?;

  let mut ws = WebSocket::after_handshake(socket);
  ws.set_writev(false);
  ws.set_auto_close(true);
  ws.set_auto_pong(true);

  // `sockdeez` lets you handle fragmentation manually
  // here we just concatenate all fragments, identical to
  // tungstenite's default behavior.
  let mut fragments = None;
  let mut fragment_opcode = OpCode::Close;

  loop {
    let frame = ws.read_frame().await?;
    match frame.opcode {
      OpCode::Close => break,
      OpCode::Text | OpCode::Binary => {
        if frame.fin {
          if fragments.is_some() {
            return Err("Invalid fragment".into());
          }
          let frame = Frame::new(true, frame.opcode, None, frame.payload);
          ws.write_frame(frame).await?;
        } else {
          fragments = match frame.opcode {
            OpCode::Text => match utf8::decode(&frame.payload) {
              Ok(text) => Some(Fragment::Text(None, text.as_bytes().to_vec())),
              Err(utf8::DecodeError::Incomplete {
                valid_prefix,
                incomplete_suffix,
              }) => Some(Fragment::Text(
                Some(incomplete_suffix),
                valid_prefix.as_bytes().to_vec(),
              )),
              Err(utf8::DecodeError::Invalid { .. }) => {
                return Err("Invalid UTF-8".into());
              }
            },
            OpCode::Binary => Some(Fragment::Binary(frame.payload)),
            _ => unreachable!(),
          };
          fragment_opcode = frame.opcode;
        }
      }
      OpCode::Continuation => match fragments.as_mut() {
        None => {
          return Err("Invalid continuation frame".into());
        }
        Some(Fragment::Text(data, input)) => {
          let mut tail = &frame.payload[..];
          if let Some(mut incomplete) = data.take() {
            if let Some((result, rest)) =
              incomplete.try_complete(&frame.payload)
            {
              tail = rest;
              match result {
                Ok(text) => {
                  input.extend_from_slice(text.as_bytes());
                }
                Err(_) => {
                  return Err("Invalid UTF-8".into());
                }
              }
            } else {
              tail = &[];
              data.replace(incomplete);
            }
          }

          match utf8::decode(tail) {
            Ok(text) => {
              input.extend_from_slice(text.as_bytes());
            }
            Err(utf8::DecodeError::Incomplete {
              valid_prefix,
              incomplete_suffix,
            }) => {
              input.extend_from_slice(valid_prefix.as_bytes());
              *data = Some(incomplete_suffix);
            }
            Err(utf8::DecodeError::Invalid { valid_prefix, .. }) => {
              input.extend_from_slice(valid_prefix.as_bytes());
              return Err("Invalid UTF-8".into());
            }
          }

          if frame.fin {
            let frame = Frame::new(
              true,
              fragment_opcode,
              None,
              fragments.take().unwrap().take_buffer(),
            );
            ws.write_frame(frame).await?;
            fragments = None;
          }
        }
        Some(Fragment::Binary(data)) => {
          data.extend_from_slice(&frame.payload);
          if frame.fin {
            let frame = Frame::new(
              true,
              fragment_opcode,
              None,
              fragments.take().unwrap().take_buffer(),
            );
            ws.write_frame(frame).await?;
            fragments = None;
          }
        }
      },
      _ => {}
    }
  }

  Ok(())
}

async fn handshake(
  mut socket: TcpStream,
) -> Result<TcpStream, Box<dyn std::error::Error + Send + Sync>> {
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
  Ok(socket)
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
