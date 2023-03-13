#[inline]
pub fn unmask(payload: &mut [u8], mask: [u8; 4]) {
    // #[cfg(not(target_arch = "aarch64"))]
    for i in 0..payload.len() {
        payload[i] ^= mask[i % 4];
    }

    // #[cfg(target_arch = "aarch64")]
    // unsafe {
    //     use std::arch::aarch64::*;
    //     let mask = vld1q_u8(mask.as_ptr());
    //     let mut i = 0;
    //     while i < payload.len() {
    //         let data = vld1q_u8(payload[i..].as_ptr());
    //         let result = veorq_u8(data, mask);
    //         vst1q_u8(payload[i..].as_mut_ptr(), result);
    //         i += 16;
    //     }
    // }
}
