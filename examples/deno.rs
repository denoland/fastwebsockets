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
use deno_core::error::AnyError;
use deno_core::op;
use deno_core::serde_v8;
use deno_core::v8;
use deno_core::JsRuntime;
use sha1::{Digest, Sha1};
use sockdeez::{Frame, OpCode, WebSocket};
use std::env;
use std::future::Future;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

async fn handle_client(
  socket: TcpStream,
  js_cb: event::JsCb,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let socket = handshake(socket).await?;

  let mut ws = WebSocket::after_handshake(socket);
  ws.set_writev(false);
  ws.set_auto_close(true);
  ws.set_auto_pong(true);

  loop {
    let frame = ws.read_frame().await?;

    match frame.opcode {
      OpCode::Close => break,
      OpCode::Text | OpCode::Binary => unsafe {
        let payload = js_cb.call(frame.payload);

        let frame = Frame::new(true, frame.opcode, None, payload);
        ws.write_frame(frame).await?;
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

#[op(v8)]
fn op_serve(
  scope: &mut v8::HandleScope,
  js_cb: serde_v8::Value,
) -> Result<impl Future<Output = Result<(), AnyError>> + 'static, AnyError> {
  let js_cb = event::JsCb::new(scope, js_cb);
  Ok(async move {
    tokio::spawn(async move {
      let listener = TcpListener::bind("127.0.0.1:8080").await?;
      println!("Server started, listening on {}", "127.0.0.1:8080");
      loop {
        let (socket, _) = listener.accept().await?;
        println!("Client connected");
        tokio::spawn(async move {
          if let Err(e) = handle_client(socket, js_cb).await {
            println!("An error occurred: {:?}", e);
          }
        });
      }
    })
    .await?
  })
}

fn create_js_runtime() -> JsRuntime {
  let ext = deno_core::Extension::builder("my_ext")
    .ops(vec![op_serve::decl()])
    .build();

  JsRuntime::new(deno_core::RuntimeOptions {
    extensions: vec![ext],
    will_snapshot: false,
    ..Default::default()
  })
}

fn main() {
  // NOTE: `--help` arg will display V8 help and exit
  deno_core::v8_set_flags(env::args().collect());

  let mut js_runtime = create_js_runtime();
  let runtime = tokio::runtime::Builder::new_current_thread()
    .enable_io()
    .build()
    .unwrap();
  let future = async move {
    js_runtime
      .execute_script("deno.js", include_str!("deno.js"))
      .unwrap();
    js_runtime.run_event_loop(false).await
  };
  runtime.block_on(future).unwrap();
}

mod event {
  use deno_core::serde_v8;
  use deno_core::serde_v8::Serializable;
  use deno_core::v8;
  use deno_core::ZeroCopyBuf;

  #[derive(Clone, Copy)]
  pub(crate) struct JsCb {
    isolate: *mut v8::Isolate,
    js_cb: *mut v8::Function,
    context: *mut v8::Context,
  }

  impl JsCb {
    pub fn new(scope: &mut v8::HandleScope, cb: serde_v8::Value) -> Self {
      let current_context = scope.get_current_context();
      let context = v8::Global::new(scope, current_context).into_raw();
      let isolate: *mut v8::Isolate = &mut *scope as &mut v8::Isolate;
      Self {
        isolate,
        js_cb: v8::Global::new(scope, cb.v8_value).into_raw().as_ptr()
          as *mut v8::Function,
        context: context.as_ptr(),
      }
    }

    // SAFETY: Must be called from the same thread as the isolate.
    pub unsafe fn call(&self, buf: Vec<u8>) -> Vec<u8> {
      let js_cb = unsafe { &mut *self.js_cb };
      let isolate = unsafe { &mut *self.isolate };
      let context = unsafe {
        std::mem::transmute::<*mut v8::Context, v8::Local<v8::Context>>(
          self.context,
        )
      };
      let recv = v8::undefined(isolate).into();
      let scope = &mut v8::HandleScope::with_context(isolate, context);
      let args = &[ZeroCopyBuf::from(buf).to_v8(scope).unwrap()];
      let to_send = js_cb.call(scope, recv, args).unwrap();
      serde_v8::from_v8::<ZeroCopyBuf>(scope, to_send)
        .unwrap()
        .to_vec()
    }
  }

  // SAFETY: JsCb is Send + Sync to bypass restrictions in tokio::spawn.
  unsafe impl Send for JsCb {}
  unsafe impl Sync for JsCb {}
}
