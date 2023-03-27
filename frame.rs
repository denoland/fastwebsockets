use tokio::io::AsyncReadExt;
use tokio::{io::AsyncWriteExt, net::TcpStream};

macro_rules! repr_u8 {
    ($(#[$meta:meta])* $vis:vis enum $name:ident {
      $($(#[$vmeta:meta])* $vname:ident $(= $val:expr)?,)*
    }) => {
      $(#[$meta])*
      $vis enum $name {
        $($(#[$vmeta])* $vname $(= $val)?,)*
      }

      #[derive(Debug)]
      pub enum Error {
        InvalidValue,
      }

      impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          match self {
            Error::InvalidValue => write!(f, "invalid value"),
          }
        }
      }

      impl std::error::Error for Error {}

      impl core::convert::TryFrom<u8> for $name {
        type Error = Error;

        fn try_from(v: u8) -> Result<Self, Self::Error> {
          match v {
            $(x if x == $name::$vname as u8 => Ok($name::$vname),)*
            _ => Err(Error::InvalidValue),
          }
        }
      }
    }
}

pub struct Frame {
  fin: bool,
  pub opcode: OpCode,
  mask: Option<[u8; 4]>,
  pub payload: Vec<u8>,
}

const MAX_HEAD_SIZE: usize = 10;

impl Frame {
  pub fn new(
    fin: bool,
    opcode: OpCode,
    mask: Option<[u8; 4]>,
    payload: Vec<u8>,
  ) -> Self {
    Self {
      fin,
      opcode,
      mask,
      payload,
    }
  }

  pub fn text(payload: Vec<u8>) -> Self {
    Self {
      fin: true,
      opcode: OpCode::Text,
      mask: None,
      payload,
    }
  }

  pub fn binary(payload: Vec<u8>) -> Self {
    Self {
      fin: true,
      opcode: OpCode::Binary,
      mask: None,
      payload,
    }
  }

  pub fn close(payload: Vec<u8>) -> Self {
    Self {
      fin: true,
      opcode: OpCode::Close,
      mask: None,
      payload,
    }
  }

  pub fn pong(payload: Vec<u8>) -> Self {
    Self {
      fin: true,
      opcode: OpCode::Pong,
      mask: None,
      payload,
    }
  }

  pub fn unmask(&mut self) {
    if let Some(mask) = self.mask {
      crate::mask::unmask(&mut self.payload, mask);
    }
  }

  pub fn fmt_head(&mut self, head: &mut [u8]) -> usize {
    head[0] = (self.fin as u8) << 7 | (self.opcode as u8);

    let len = self.payload.len();
    if len < 126 {
      head[1] = len as u8;
      2
    } else if len < 65536 {
      head[1] = 126;
      head[2..4].copy_from_slice(&(len as u16).to_be_bytes());
      4
    } else {
      head[1] = 127;
      head[2..10].copy_from_slice(&(len as u64).to_be_bytes());
      10
    }
  }

  pub async fn writev<S>(
    &mut self,
    stream: &mut S,
  ) -> Result<(), std::io::Error>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    use std::io::IoSlice;

    match self.opcode {
      OpCode::Text => {
        let mut head = [0; MAX_HEAD_SIZE];
        let size = self.fmt_head(&mut head);

        stream
          .write_vectored(&[
            IoSlice::new(&head[..size]),
            IoSlice::new(&self.payload),
          ])
          .await?;

        Ok(())
      }
      _ => todo!(),
    }
  }

  pub fn write<'a>(&mut self, buf: &'a mut Vec<u8>) -> &'a [u8] {
    fn reserve_enough(buf: &mut Vec<u8>, len: usize) {
      if buf.len() < len {
        buf.resize(len, 0);
      }
    }
    let len = self.payload.len();
    reserve_enough(buf, len + MAX_HEAD_SIZE);

    let size = self.fmt_head(buf);
    buf[size..size + len].copy_from_slice(&self.payload);
    &buf[..size + len]
    // match self.opcode {
    //     OpCode::Text | OpCode::Binary | OpCode::Close => {
    //         let len = self.payload.len();
    //         reserve_enough(buf, len + MAX_HEAD_SIZE);

    //         let size = self.fmt_head(buf);
    //         buf[size..size + len].copy_from_slice(&self.payload);
    //         &buf[..size + len]
    //     },
    //     _ => todo!(),
    // }
  }
}

repr_u8! {
    #[repr(u8)]
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub enum OpCode {
        Continuation = 0x0,
        Text = 0x1,
        Binary = 0x2,
        Close = 0x8,
        Ping = 0x9,
        Pong = 0xA,
    }
}
