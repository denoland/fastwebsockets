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

use crate::frame::Frame;
use crate::OpCode;
use crate::WebSocket;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

pub enum Fragment {
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

pub struct FragmentCollector<S> {
  ws: WebSocket<S>,
  fragments: Option<Fragment>,
  opcode: OpCode,
}

impl<S> FragmentCollector<S> {
  pub fn new(ws: WebSocket<S>) -> FragmentCollector<S>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    FragmentCollector {
      ws,
      fragments: None,
      opcode: OpCode::Close,
    }
  }

  pub async fn read_frame(
    &mut self,
  ) -> Result<Frame, Box<dyn std::error::Error + Send + Sync>>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    loop {
      let frame = self.ws.read_frame().await?;
      match frame.opcode {
        OpCode::Text | OpCode::Binary => {
          if frame.fin {
            if self.fragments.is_some() {
              return Err("Invalid fragment".into());
            }
            return Ok(Frame::new(true, frame.opcode, None, frame.payload));
          } else {
            self.fragments = match frame.opcode {
              OpCode::Text => match utf8::decode(&frame.payload) {
                Ok(text) => {
                  Some(Fragment::Text(None, text.as_bytes().to_vec()))
                }
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
            self.opcode = frame.opcode;
          }
        }
        OpCode::Continuation => match self.fragments.as_mut() {
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
              return Ok(Frame::new(
                true,
                self.opcode,
                None,
                self.fragments.take().unwrap().take_buffer(),
              ));
            }
          }
          Some(Fragment::Binary(data)) => {
            data.extend_from_slice(&frame.payload);
            if frame.fin {
              return Ok(Frame::new(
                true,
                self.opcode,
                None,
                self.fragments.take().unwrap().take_buffer(),
              ));
            }
          }
        },
        _ => return Ok(frame),
      }
    }
  }

  #[inline]
  pub async fn write_frame(
    &mut self,
    frame: Frame,
  ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    self.ws.write_frame(frame).await?;
    Ok(())
  }

  #[inline]
  pub fn try_write(
    &mut self,
    frame: Frame,
    cb: impl FnOnce(&mut S, &[u8]) -> std::io::Result<usize>,
  ) -> bool {
    self.ws.try_write(frame, cb)
  }
}
