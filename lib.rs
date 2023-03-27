mod frame;
mod mask;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

pub use crate::frame::Frame;
pub use crate::frame::OpCode;

pub struct WebSocket<S> {
  stream: S,
  write_buffer: Vec<u8>,
  read_buffer: Option<Vec<u8>>,
  vectored: bool,
  auto_close: bool,
  auto_pong: bool,
}

impl<S> WebSocket<S> {
  pub fn after_handshake(stream: S) -> Self
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    Self {
      stream,
      write_buffer: Vec::with_capacity(2),
      read_buffer: None,
      vectored: false,
      auto_close: true,
      auto_pong: true,
    }
  }

  pub fn set_writev(&mut self, vectored: bool) {
    self.vectored = vectored;
  }

  pub fn set_auto_close(&mut self, auto_close: bool) {
    self.auto_close = auto_close;
  }

  pub fn set_auto_pong(&mut self, auto_pong: bool) {
    self.auto_pong = auto_pong;
  }

  pub async fn write_frame(
    &mut self,
    mut frame: Frame,
  ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    if self.vectored {
      frame.writev(&mut self.stream).await?;
    } else {
      let text = frame.write(&mut self.write_buffer);
      self.stream.write_all(text).await?;
    }

    Ok(())
  }

  pub async fn read_frame(
    &mut self,
  ) -> Result<Frame, Box<dyn std::error::Error + Send + Sync>>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    loop {
      let mut frame = self.parse_frame_header().await?;
      frame.unmask();

      match frame.opcode {
        OpCode::Close if self.auto_close => {
          self
            .write_frame(Frame::close(frame.payload.clone()))
            .await?;
          break Ok(frame);
        }
        OpCode::Ping if self.auto_pong => {
          dbg!("Ping");
          self.write_frame(Frame::pong(frame.payload)).await?;
        }
        OpCode::Pong => {
          dbg!("Pong");
        }
        OpCode::Continuation => {
          dbg!("Continuation");
        }
        _ => break Ok(frame),
      }
    }
  }

  async fn parse_frame_header(
    &mut self,
  ) -> Result<Frame, Box<dyn std::error::Error + Send + Sync>>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
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
