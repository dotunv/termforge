//! Notify-only signed update discovery (ADR 0013).

use anyhow::{Context, Result};

const MANIFEST_URL: &str =
    "https://github.com/dotunv/termforge/releases/latest/download/manifest.json";
const SIGNATURE_URL: &str =
    "https://github.com/dotunv/termforge/releases/latest/download/manifest.json.sig";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateNotice {
    pub version: String,
    pub notes: String,
    pub url: String,
}

/// Fetch and authenticate the release feed. Builds without an embedded key
/// intentionally do no network I/O and cannot offer updates.
pub fn check() -> Result<Option<UpdateNotice>> {
    let Some(key_hex) = option_env!("TERMFORGE_UPDATE_PUBLIC_KEY")
        .map(str::trim)
        .filter(|key| !key.is_empty())
    else {
        return Ok(None);
    };
    let manifest = fetch(MANIFEST_URL)?;
    let signature =
        String::from_utf8(fetch(SIGNATURE_URL)?).context("update signature is not UTF-8")?;
    evaluate(
        &manifest,
        &signature,
        key_hex,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

fn fetch(url: &str) -> Result<Vec<u8>> {
    let mut response = ureq::get(url)
        .header(
            "User-Agent",
            concat!("TermForge/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .with_context(|| format!("fetch {url}"))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_MANIFEST_BYTES)
        .read_to_vec()
        .with_context(|| format!("read {url}"))?;
    Ok(bytes)
}

fn evaluate(
    manifest_bytes: &[u8],
    signature: &str,
    key_hex: &str,
    current: &str,
    os: &str,
    arch: &str,
) -> Result<Option<UpdateNotice>> {
    let key = tf_update::parse_public_key(key_hex).context("embedded update key")?;
    let manifest = tf_update::verify_and_parse(manifest_bytes, signature, &key)
        .context("verify update manifest")?;
    Ok(manifest
        .available(current, os, arch)
        .context("select update asset")?
        .map(|available| UpdateNotice {
            version: available.version.to_owned(),
            notes: available.notes.to_owned(),
            url: available.asset.url.clone(),
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_newer_release_becomes_a_notice() {
        let seed = "11".repeat(32);
        let key = tf_update::public_key_hex(&seed).unwrap();
        let manifest = br#"{"version":"9.0.0","notes":"Safer panes","assets":[{"os":"linux","arch":"x86_64","url":"https://example.com/termforge.tar.gz","sha256":"0000000000000000000000000000000000000000000000000000000000000000","size":1}]}"#;
        let signature = tf_update::sign(manifest, &seed).unwrap();
        let notice = evaluate(manifest, &signature, &key, "1.0.0", "linux", "x86_64")
            .unwrap()
            .unwrap();
        assert_eq!(notice.version, "9.0.0");
        assert_eq!(notice.notes, "Safer panes");
    }
}
