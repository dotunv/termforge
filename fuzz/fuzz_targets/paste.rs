#![no_main]
use libfuzzer_sys::fuzz_target;
use tf_input::{encode_paste, InputModes};

// With bracketed paste on, the only terminator in the output is the final
// one: pasted text can never close the bracket early and inject keystrokes.
fuzz_target!(|text: &str| {
    let modes = InputModes {
        app_cursor: false,
        bracketed_paste: true,
    };
    let out = encode_paste(text, modes);
    let body = out
        .strip_prefix(b"\x1b[200~".as_slice())
        .and_then(|rest| rest.strip_suffix(b"\x1b[201~".as_slice()))
        .expect("bracketed output must be wrapped");
    assert!(!body.windows(6).any(|w| w == b"\x1b[201~" || w == b"\x1b[200~"));

    // Unbracketed output never contains a bare line feed.
    let plain = encode_paste(text, InputModes::default());
    assert!(!plain.contains(&b'\n'));
});
