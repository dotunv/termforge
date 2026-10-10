#![no_main]
use libfuzzer_sys::fuzz_target;

// User-edited config is hot-reloaded, so parsing must never panic, and any
// config that is accepted must survive a write/read cycle unchanged.
fuzz_target!(|text: &str| {
    let Ok(config) = tf_config::Config::parse(text) else { return };
    let written = toml::to_string_pretty(&config).expect("accepted config serialises");
    let again = tf_config::Config::parse(&written).expect("written config re-parses");
    assert_eq!(
        toml::to_string_pretty(&again).unwrap(),
        written,
        "config did not round-trip"
    );
});
