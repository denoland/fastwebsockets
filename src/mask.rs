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

#[inline]
fn unmask_easy(payload: &mut [u8], mask: [u8; 4]) {
  payload.iter_mut().enumerate().for_each(|(i, v)| {
    *v ^= mask[i & 3];
  });
}

// TODO(@littledivy): Compiler does a good job at auto-vectorizing `unmask_fallback` with
// -C target-cpu=native. Below is a manual implementation.
//
// #[cfg(all(target_arch = "x86_64", feature = "simd"))]
// #[inline]
// fn unmask_x86_64(payload: &mut [u8], mask: [u8; 4]) {
//   #[inline]
//   fn sse2(payload: &mut [u8], mask: [u8; 4]) {
//     const ALIGNMENT: usize = 16;
//     unsafe {
//       use std::arch::x86_64::*;
//
//       let len = payload.len();
//       if len < ALIGNMENT {
//         return unmask_fallback(payload, mask);
//       }
//
//       let start = len - len % ALIGNMENT;
//
//       let mut aligned_mask = [0; ALIGNMENT];
//
//       for j in (0..ALIGNMENT).step_by(4) {
//         aligned_mask[j] = mask[j % 4];
//         aligned_mask[j + 1] = mask[(j % 4) + 1];
//         aligned_mask[j + 2] = mask[(j % 4) + 2];
//         aligned_mask[j + 3] = mask[(j % 4) + 3];
//       }
//
//       let mask_m = _mm_loadu_si128(aligned_mask.as_ptr() as *const _);
//
//       for index in (0..start).step_by(ALIGNMENT) {
//         let ptr = payload.as_mut_ptr().add(index);
//         let mut v = _mm_loadu_si128(ptr as *const _);
//         v = _mm_xor_si128(v, mask_m);
//         _mm_storeu_si128(ptr as *mut _, v);
//       }
//
//       if len != start {
//         unmask_fallback(&mut payload[start..], mask);
//       }
//     }
//   }
//   #[cfg(target_feature = "sse2")]
//   {
//     return sse2(payload, mask);
//   }
//
//   #[cfg(not(target_feature = "sse2"))]
//   {
//     use core::mem;
//     use std::sync::atomic::AtomicPtr;
//     use std::sync::atomic::Ordering;
//
//     type FnRaw = *mut ();
//     type FnImpl = unsafe fn(&mut [u8], [u8; 4]);
//
//     unsafe fn get_impl(input: &mut [u8], mask: [u8; 4]) {
//       let fun = if std::is_x86_feature_detected!("sse2") {
//         sse2
//       } else {
//         unmask_fallback
//       };
//       FN.store(fun as FnRaw, Ordering::Relaxed);
//       (fun)(input, mask);
//     }
//
//     static FN: AtomicPtr<()> = AtomicPtr::new(get_impl as FnRaw);
//
//     if payload.len() < 16 {
//       return unmask_fallback(payload, mask);
//     }
//
//     let fun = FN.load(Ordering::Relaxed);
//     unsafe { mem::transmute::<FnRaw, FnImpl>(fun)(payload, mask) }
//   }
// }

// Faster version of `unmask_easy()` which operates on 4-byte blocks.
// https://github.com/snapview/tungstenite-rs/blob/e5efe537b87a6705467043fe44bb220ddf7c1ce8/src/protocol/frame/mask.rs#L23
//
// https://godbolt.org/z/EPTYo5jK8
#[inline]
fn unmask_fallback(buf: &mut [u8], mask: [u8; 4]) {
  let mask_u32 = u32::from_ne_bytes(mask);

  let (prefix, words, suffix) = unsafe { buf.align_to_mut::<u32>() };
  unmask_easy(prefix, mask);
  let head = prefix.len() & 3;
  let mask_u32 = if head > 0 {
    if cfg!(target_endian = "big") {
      mask_u32.rotate_left(8 * head as u32)
    } else {
      mask_u32.rotate_right(8 * head as u32)
    }
  } else {
    mask_u32
  };
  for word in words.iter_mut() {
    *word ^= mask_u32;
  }
  unmask_easy(suffix, mask_u32.to_ne_bytes());
}

/// Unmask a payload using the given 4-byte mask.
#[inline]
pub fn unmask(payload: &mut [u8], mask: [u8; 4]) {
  unmask_fallback(payload, mask)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_unmask() {
    let mut payload = [0u8; 33];
    let mask = [1, 2, 3, 4];
    unmask(&mut payload, mask);
    assert_eq!(
      &payload,
      &[
        1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4,
        1, 2, 3, 4, 1, 2, 3, 4, 1
      ]
    );
  }

  #[test]
  fn length_variation_unmask() {
    for len in &[0, 2, 3, 8, 16, 18, 31, 32, 40] {
      let mut payload = vec![0u8; *len];
      let mask = [1, 2, 3, 4];
      unmask(&mut payload, mask);

      let expected = (0..*len).map(|i| (i & 3) as u8 + 1).collect::<Vec<_>>();
      assert_eq!(payload, expected);
    }
  }

  #[test]
  fn length_variation_unmask_2() {
    for len in &[0, 2, 3, 8, 16, 18, 31, 32, 40] {
      let mut payload = vec![0u8; *len];
      let mask = rand::random::<[u8; 4]>();
      unmask(&mut payload, mask);

      let expected = (0..*len).map(|i| mask[i & 3]).collect::<Vec<_>>();
      assert_eq!(payload, expected);
    }
  }
}
