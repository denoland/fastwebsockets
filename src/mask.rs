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

#[cfg(not(target_arch = "aarch64"))]
fn unmask_easy(payload: &mut [u8], mask: [u8; 4]) {
  for i in 0..payload.len() {
    payload[i] ^= mask[i & 3];
  }
}

#[inline]
pub fn unmask(payload: &mut [u8], mask: [u8; 4]) {
  #[cfg(not(target_arch = "aarch64"))]
  return unmask_easy(payload, mask);

  #[cfg(target_arch = "aarch64")]
  unsafe {
    // ~10% faster on small payloads (1024)
    // ~30% faster on largest default payload (64 << 20)
    extern "C" {
      fn unmask(payload: *mut u8, mask: *const u8, len: usize);
    }

    unmask(payload.as_mut_ptr(), mask.as_ptr(), payload.len());
  }
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
