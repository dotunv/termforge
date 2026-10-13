#![no_main]
use libfuzzer_sys::fuzz_target;

// Theme files are dropped into a folder by users, often downloaded, so
// parsing must never panic and an accepted theme must always build.
fuzz_target!(|text: &str| {
    let Ok(spec) = tf_config::theme::ThemeSpec::parse(text) else { return };
    let _ = spec.build();
    let written = toml::to_string_pretty(&spec).expect("accepted theme serialises");
    let again = tf_config::theme::ThemeSpec::parse(&written).expect("written theme re-parses");
    assert_eq!(again, spec, "theme did not round-trip");
});
