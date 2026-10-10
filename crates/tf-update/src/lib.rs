//! Signed update manifests (ADR 0013).
//!
//! This crate is deliberately transport-free: it takes bytes in and says
//! whether they are an authentic, newer release for this platform. Fetching
//! the manifest and installing an artifact are the caller's job, and neither
//! may happen on the strength of anything this crate has not verified.

use std::cmp::Ordering;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UpdateError {
    #[error("signature is not valid hex of the expected length")]
    BadSignatureEncoding,
    #[error("public key is not valid hex of the expected length")]
    BadKeyEncoding,
    #[error("manifest signature does not match")]
    BadSignature,
    #[error("manifest is not valid: {0}")]
    BadManifest(String),
    #[error("`{0}` is not a valid version")]
    BadVersion(String),
    #[error("asset URL `{0}` is not https")]
    InsecureUrl(String),
    #[error("downloaded asset does not match its published checksum")]
    ChecksumMismatch,
    #[error("downloaded asset is {actual} bytes, manifest says {expected}")]
    SizeMismatch { expected: u64, actual: u64 },
}

pub type Result<T> = std::result::Result<T, UpdateError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// `std::env::consts::OS` value: `windows`, `macos`, `linux`.
    pub os: String,
    /// `std::env::consts::ARCH` value: `x86_64`, `aarch64`.
    pub arch: String,
    pub url: String,
    /// Lowercase hex SHA-256 of the file at `url`.
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub assets: Vec<Asset>,
}

/// A newer release that applies to this platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available<'a> {
    pub version: &'a str,
    pub notes: &'a str,
    pub asset: &'a Asset,
}

fn from_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.trim();
    if text.len() != N * 2 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Parse a hex-encoded Ed25519 public key.
pub fn parse_public_key(hex: &str) -> Result<VerifyingKey> {
    let bytes = from_hex::<32>(hex).ok_or(UpdateError::BadKeyEncoding)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| UpdateError::BadKeyEncoding)
}

/// Verify `manifest` against its detached hex signature and parse it. The
/// signature covers the exact bytes, so callers must not re-serialise first.
pub fn verify_and_parse(
    manifest: &[u8],
    signature_hex: &str,
    key: &VerifyingKey,
) -> Result<Manifest> {
    let sig = from_hex::<64>(signature_hex).ok_or(UpdateError::BadSignatureEncoding)?;
    key.verify(manifest, &Signature::from_bytes(&sig))
        .map_err(|_| UpdateError::BadSignature)?;
    let parsed: Manifest =
        serde_json::from_slice(manifest).map_err(|e| UpdateError::BadManifest(e.to_string()))?;
    Version::parse(&parsed.version)?;
    for asset in &parsed.assets {
        if !asset.url.starts_with("https://") {
            return Err(UpdateError::InsecureUrl(asset.url.clone()));
        }
        if from_hex::<32>(&asset.sha256).is_none() {
            return Err(UpdateError::BadManifest(format!(
                "asset {} has a malformed sha256",
                asset.url
            )));
        }
    }
    Ok(parsed)
}

/// Sign manifest bytes with a hex-encoded 32-byte seed. Used by release
/// tooling; the seed never ships in the application.
pub fn sign(manifest: &[u8], seed_hex: &str) -> Result<String> {
    let seed = from_hex::<32>(seed_hex).ok_or(UpdateError::BadKeyEncoding)?;
    let key = SigningKey::from_bytes(&seed);
    Ok(to_hex(&key.sign(manifest).to_bytes()))
}

/// The hex public key for a signing seed, to embed in the application.
pub fn public_key_hex(seed_hex: &str) -> Result<String> {
    let seed = from_hex::<32>(seed_hex).ok_or(UpdateError::BadKeyEncoding)?;
    Ok(to_hex(
        SigningKey::from_bytes(&seed).verifying_key().as_bytes(),
    ))
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

impl Manifest {
    /// The newer release for this platform, if there is one. Equal or older
    /// versions are never offered, so a replayed old manifest cannot
    /// downgrade an installation.
    pub fn available(&self, current: &str, os: &str, arch: &str) -> Result<Option<Available<'_>>> {
        let current = Version::parse(current)?;
        let latest = Version::parse(&self.version)?;
        if latest <= current {
            return Ok(None);
        }
        Ok(self
            .assets
            .iter()
            .find(|a| a.os == os && a.arch == arch)
            .map(|asset| Available {
                version: &self.version,
                notes: &self.notes,
                asset,
            }))
    }
}

impl Asset {
    /// Check downloaded bytes against the signed size and checksum.
    pub fn verify(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 != self.size {
            return Err(UpdateError::SizeMismatch {
                expected: self.size,
                actual: bytes.len() as u64,
            });
        }
        if !sha256_hex(bytes).eq_ignore_ascii_case(&self.sha256) {
            return Err(UpdateError::ChecksumMismatch);
        }
        Ok(())
    }
}

/// A semantic version, enough of SemVer 2.0 to order TermForge's
/// `0.1.0-beta.1` style tags correctly (build metadata is ignored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: Vec<Ident>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    // Numeric identifiers sort below alphanumeric ones.
    Num(u64),
    Text(String),
}

impl Version {
    pub fn parse(text: &str) -> Result<Self> {
        let bad = || UpdateError::BadVersion(text.to_owned());
        let text = text.trim().trim_start_matches('v');
        let text = text.split('+').next().unwrap_or(text);
        let (core, pre) = match text.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let mut nums = [0u64; 3];
        for n in &mut nums {
            *n = parts.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        }
        if parts.next().is_some() {
            return Err(bad());
        }
        let pre = match pre {
            None => Vec::new(),
            Some(p) => p
                .split('.')
                .map(|id| {
                    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                        return Err(bad());
                    }
                    Ok(match id.parse::<u64>() {
                        Ok(n) => Ident::Num(n),
                        Err(_) => Ident::Text(id.to_owned()),
                    })
                })
                .collect::<Result<_>>()?,
        };
        Ok(Self { core: nums, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A release outranks its own pre-releases.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn manifest_json(version: &str) -> Vec<u8> {
        let asset = Asset {
            os: "linux".into(),
            arch: "x86_64".into(),
            url: "https://example.com/termforge.tar.gz".into(),
            sha256: sha256_hex(b"payload"),
            size: 7,
        };
        serde_json::to_vec(&Manifest {
            version: version.into(),
            notes: "notes".into(),
            assets: vec![asset],
        })
        .unwrap()
    }

    fn key() -> VerifyingKey {
        parse_public_key(&public_key_hex(SEED).unwrap()).unwrap()
    }

    #[test]
    fn signed_manifest_verifies() {
        let bytes = manifest_json("0.2.0");
        let sig = sign(&bytes, SEED).unwrap();
        let m = verify_and_parse(&bytes, &sig, &key()).unwrap();
        let a = m.available("0.1.0", "linux", "x86_64").unwrap().unwrap();
        assert_eq!(a.version, "0.2.0");
        a.asset.verify(b"payload").unwrap();
    }

    #[test]
    fn tampering_is_rejected() {
        let bytes = manifest_json("0.2.0");
        let sig = sign(&bytes, SEED).unwrap();
        let mut forged = bytes.clone();
        let at = forged.iter().position(|b| *b == b'2').unwrap();
        forged[at] = b'9';
        assert_eq!(
            verify_and_parse(&forged, &sig, &key()).unwrap_err(),
            UpdateError::BadSignature
        );
        let other = public_key_hex(&"11".repeat(32)).unwrap();
        assert_eq!(
            verify_and_parse(&bytes, &sig, &parse_public_key(&other).unwrap()).unwrap_err(),
            UpdateError::BadSignature
        );
    }

    #[test]
    fn malformed_signature_and_key_are_errors_not_panics() {
        let bytes = manifest_json("0.2.0");
        for bad in [
            "",
            "zz",
            &"g".repeat(128),
            &"ab".repeat(63),
            "é".repeat(64).as_str(),
        ] {
            assert_eq!(
                verify_and_parse(&bytes, bad, &key()).unwrap_err(),
                UpdateError::BadSignatureEncoding
            );
        }
        assert!(parse_public_key("nothex").is_err());
    }

    #[test]
    fn insecure_urls_are_rejected_even_when_signed() {
        let mut m: Manifest = serde_json::from_slice(&manifest_json("0.2.0")).unwrap();
        m.assets[0].url = "http://example.com/x".into();
        let bytes = serde_json::to_vec(&m).unwrap();
        let sig = sign(&bytes, SEED).unwrap();
        assert!(matches!(
            verify_and_parse(&bytes, &sig, &key()),
            Err(UpdateError::InsecureUrl(_))
        ));
    }

    #[test]
    fn never_offers_same_or_older_versions() {
        let m: Manifest = serde_json::from_slice(&manifest_json("0.2.0")).unwrap();
        assert!(m.available("0.2.0", "linux", "x86_64").unwrap().is_none());
        assert!(m.available("0.3.0", "linux", "x86_64").unwrap().is_none());
        assert!(m.available("0.1.9", "windows", "x86_64").unwrap().is_none());
        assert!(m.available("not-a-version", "linux", "x86_64").is_err());
    }

    #[test]
    fn asset_verification_checks_size_then_hash() {
        let m: Manifest = serde_json::from_slice(&manifest_json("0.2.0")).unwrap();
        let a = &m.assets[0];
        assert!(a.verify(b"payload").is_ok());
        assert_eq!(
            a.verify(b"payloaD").unwrap_err(),
            UpdateError::ChecksumMismatch
        );
        assert!(matches!(
            a.verify(b"short").unwrap_err(),
            UpdateError::SizeMismatch { .. }
        ));
    }

    #[test]
    fn prerelease_ordering_follows_semver() {
        let order = [
            "0.1.0-alpha",
            "0.1.0-alpha.1",
            "0.1.0-beta.1",
            "0.1.0-beta.2",
            "0.1.0-beta.11",
            "0.1.0-rc.1",
            "0.1.0",
            "0.1.1",
            "0.2.0",
            "1.0.0",
        ];
        for w in order.windows(2) {
            let (a, b) = (Version::parse(w[0]).unwrap(), Version::parse(w[1]).unwrap());
            assert!(a < b, "{} < {}", w[0], w[1]);
        }
        assert_eq!(
            Version::parse("v0.1.0+build5").unwrap(),
            Version::parse("0.1.0").unwrap()
        );
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1.2.3-",
            "1.2.3-a..b",
            "-1.0.0",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad:?}");
        }
    }
}
