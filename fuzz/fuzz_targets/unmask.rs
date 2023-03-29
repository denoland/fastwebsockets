#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut data = data.to_vec();
    sockdeez::unmask(&mut data, [1, 2, 3, 4]);
});