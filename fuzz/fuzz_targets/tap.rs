#![no_main]
use libfuzzer_sys::fuzz_target;

// Any split of any input must produce the same events as the whole input.
fuzz_target!(|data: &[u8]| {
    let Some((&split, rest)) = data.split_first() else { return };
    let split = split as usize % (rest.len() + 1);
    let mut whole = Vec::new();
    tf_tap::Tap::new().feed(rest, &mut whole);
    let mut tap = tf_tap::Tap::new();
    let mut parts = Vec::new();
    tap.feed(&rest[..split], &mut parts);
    tap.feed(&rest[split..], &mut parts);
    let a: Vec<_> = whole.into_iter().map(|l| l.event).collect();
    let b: Vec<_> = parts.into_iter().map(|l| l.event).collect();
    assert_eq!(a, b);
});
