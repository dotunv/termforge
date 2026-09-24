#![no_main]
use libfuzzer_sys::fuzz_target;

// Untrusted bytes on the pipe must never panic or over-allocate.
fuzz_target!(|data: &[u8]| {
    let mut r = data;
    while let Ok(Some(_)) = tf_proto::read_frame::<_, tf_proto::ClientMsg>(&mut r) {}
});
