use base64;
use frame::OpCode;
use sha1::{Digest, Sha1};
use std::convert::TryInto;
use std::fmt::Display;
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter,
};
use tokio::net::{TcpListener, TcpStream};

mod frame;
mod mask;

use frame::Frame;

struct WebSocket {
    stream: TcpStream,
    write_buffer: Vec<u8>,
    read_buffer: Option<Vec<u8>>,
    vectored: bool,
}

impl WebSocket {
    pub fn after_handshake(stream: TcpStream) -> Self {
        Self {
            stream,
            write_buffer: Vec::with_capacity(2),
            read_buffer: None,
            vectored: false,
        }
    }

    pub fn set_writev(&mut self, vectored: bool) {
        self.vectored = vectored;
    }

    pub async fn write_frame(
        &mut self,
        mut frame: Frame,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.vectored {
            frame.writev(&mut self.stream).await?;
        } else {
            let text = frame.write(&mut self.write_buffer);
            self.stream
                .write_all(text)
                .await?;       
        }

        Ok(())
    }

    pub async fn read_frame(
        &mut self,
    ) -> Result<Frame, Box<dyn std::error::Error + Send + Sync>> {
        loop {
            let mut frame = self.parse_frame_header().await?;
            frame.unmask();
    
            match frame.opcode {
                OpCode::Close => {
                    // Close.
                    self
                        .write_frame(Frame::close(frame.payload.clone()))
                        .await?;
                    break Ok(frame);
                },
                OpCode::Ping => {
                  dbg!("Ping");  
                  self
                    .write_frame(Frame::pong(frame.payload))
                    .await?;
                },
                OpCode::Pong => {
                    dbg!("Pong");
                },
                OpCode::Continuation => {
                    dbg!("Continuation");
                },
                _ => break Ok(frame),
            }
        }
    }

    async fn parse_frame_header(
        &mut self,
    ) -> Result<Frame, Box<dyn std::error::Error + Send + Sync>> {
        let mut head = [0; 2 + 4 + 100];

        let mut nread = 0;

        if let Some(buffer) = self.read_buffer.take() {
            head[..buffer.len()].copy_from_slice(&buffer);
            nread = buffer.len();
        }

        while nread < 2 {
            nread += self.stream.read(&mut head[nread..]).await?;
        }

        let fin = head[0] & 0b10000000 != 0;
        let opcode = frame::OpCode::try_from(head[0] & 0b00001111)?;
        let masked = head[1] & 0b10000000 != 0;

        let length_code = head[1] & 0x7F;
        let extra = match length_code {
            126 => 2,
            127 => 8,
            _ => 0,
        };

        let length: usize = if extra > 0 {
            while nread < 2 + extra {
                nread += self.stream.read(&mut head[nread..]).await?;
            }

            match extra {
                2 => u16::from_be_bytes(head[2..4].try_into().unwrap()) as usize,
                8 => usize::from_be_bytes(head[2..10].try_into().unwrap()),
                _ => unreachable!(),
            }
        } else {
            usize::from(length_code)
        };


        let mask = match masked {
            true => {
                while nread < 2 + extra + 4 {
                    nread += self.stream.read(&mut head[nread..]).await?;
                }

                Some(head[2 + extra..2 + extra + 4].try_into().unwrap())
            }
            false => None,
        };

        let required = 2 + extra + mask.map(|_| 4).unwrap_or(0) + length;
        dbg!(required);
        if required > head.len() {
            // Allocate more space
            let mut new_head = head.to_vec();
            new_head.resize(required, 0);

            self.stream.read_exact(&mut new_head[nread..]).await?;

            return Ok(Frame::new(
                fin,
                opcode,
                mask,
                new_head[required - length..].to_vec(),
            ));
        }

        if nread > required {
            // We read too much
            self.read_buffer = Some(head[required..nread].to_vec()); 
        }

        Ok(Frame::new(
            fin,
            opcode,
            mask,
            head[required - length..required].to_vec(),
        ))
    }
}

async fn handle_client(
    mut socket: TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Handshake
    // Read the request
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

    // Parse the headers and extract the key
    let key = extract_key(headers)?;
    let response = generate_response(&key);
    socket.write_all(response.as_bytes()).await?;

    let mut ws = WebSocket::after_handshake(socket);

    loop {
        let frame = ws.read_frame().await?;

        if let OpCode::Close = frame.opcode {
            break;
        }

        if let OpCode::Text = frame.opcode {
            let frame = Frame::text(frame.payload);
            // dbg!(String::from_utf8(frame.payload.clone()).unwrap());
            ws.write_frame(frame).await?;
        } else if let OpCode::Binary = frame.opcode {
            let frame = Frame::binary(frame.payload);
            ws.write_frame(frame).await?;
        }
    }

    Ok(())
}

fn extract_key(request: Vec<String>) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
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
    let encoded = base64::encode(&result[..]);
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
