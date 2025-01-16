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

use core::ptr::NonNull;
use std::cell::RefCell;

// 512 KiB
const RECV_SIZE: usize = 524288;

thread_local! {
    static RECV_BUF: RefCell<SharedRecv> = const { RefCell::new(SharedRecv::null()) };
}

pub(crate) fn init_once() -> &'static mut [u8] {
  RECV_BUF.with(|recv_buf| {
    let mut recv_buf = recv_buf.borrow_mut();
    recv_buf.init();
    recv_buf.get_mut()
  })
}

#[repr(transparent)]
pub struct SharedRecv {
  inner: Option<NonNull<u8>>,
}

impl SharedRecv {
  pub(crate) const fn null() -> Self {
    Self { inner: None }
  }

  pub(crate) fn init(&mut self) {
    match self.inner.as_mut() {
      Some(_) => {}
      None => {
        let mut vec = vec![0; RECV_SIZE];
        let ptr = vec.as_mut_ptr();
        std::mem::forget(vec);

        unsafe { self.inner = Some(NonNull::new_unchecked(ptr)) };
      }
    }
  }

  pub(crate) fn get_mut(&self) -> &'static mut [u8] {
    unsafe {
      std::slice::from_raw_parts_mut(self.inner.unwrap().as_ptr(), RECV_SIZE)
    }
  }
}

unsafe impl Send for SharedRecv {}
