#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| rstreamkit_fuzz::exercise("hls", data));
