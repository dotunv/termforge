//! Trust-on-first-use (TOFU) host-key store.
//!
//! Fingerprints are stored one per line as `host:port SHA256:<base64>` at
//! `%APPDATA%\TermForge\known_hosts` on Windows,
//! `~/.config/termforge/known_hosts` on other platforms.
//!
//! The first connection to a host pins its key fingerprint.  Subsequent
//! connections must present the same key; a mismatch aborts the connection
//! (possible man-in-the-middle).

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Outcome of checking a server key against the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCheck {
    /// Fingerprint matches the pinned entry.
    Known,
    /// No entry for this host yet — caller should pin and proceed (TOFU).
    Unknown,
    /// Entry exists but the fingerprint differs — reject the connection.
    Mismatch,
}

pub struct KnownHosts {
    path: PathBuf,
    /// (host:port, fingerprint) pairs in file order.
    entries: Vec<(String, String)>,
}

impl KnownHosts {
    /// Load the store from the default location (empty if the file is absent).
    pub fn load() -> Result<Self> {
        Self::load_from(default_path())
    }

    pub fn load_from(path: PathBuf) -> Result<Self> {
        let entries = if path.exists() {
            let text = std::fs::read_to_string(&path).context("read known_hosts")?;
            text.lines()
                .filter_map(|line| {
                    let mut it = line.split_whitespace();
                    match (it.next(), it.next()) {
                        (Some(host), Some(fp)) => Some((host.to_string(), fp.to_string())),
                        _ => None,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self { path, entries })
    }

    pub fn check(&self, host: &str, port: u16, fingerprint: &str) -> KeyCheck {
        let key = host_key(host, port);
        match self.entries.iter().find(|(h, _)| *h == key) {
            Some((_, fp)) if fp == fingerprint => KeyCheck::Known,
            Some(_) => KeyCheck::Mismatch,
            None => KeyCheck::Unknown,
        }
    }

    /// Pin a fingerprint for a host and persist the store.
    pub fn pin(&mut self, host: &str, port: u16, fingerprint: &str) -> Result<()> {
        let key = host_key(host, port);
        self.entries.retain(|(h, _)| *h != key);
        self.entries.push((key, fingerprint.to_string()));
        self.save()
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).context("create config dir")?;
        }
        let text: String = self
            .entries
            .iter()
            .map(|(h, fp)| format!("{h} {fp}\n"))
            .collect();
        std::fs::write(&self.path, text).context("write known_hosts")?;
        Ok(())
    }
}

fn host_key(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

fn default_path() -> PathBuf {
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        appdata.join("TermForge").join("known_hosts")
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".config").join("termforge").join("known_hosts")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> KnownHosts {
        let dir = std::env::temp_dir().join(format!("tf-kh-{}", uuid::Uuid::new_v4()));
        KnownHosts::load_from(dir.join("known_hosts")).unwrap()
    }

    #[test]
    fn tofu_pin_and_match() {
        let mut kh = temp_store();
        assert_eq!(kh.check("example.com", 22, "SHA256:abc"), KeyCheck::Unknown);
        kh.pin("example.com", 22, "SHA256:abc").unwrap();
        assert_eq!(kh.check("example.com", 22, "SHA256:abc"), KeyCheck::Known);
        assert_eq!(kh.check("example.com", 22, "SHA256:zzz"), KeyCheck::Mismatch);
        // Different port = different identity.
        assert_eq!(kh.check("example.com", 2222, "SHA256:abc"), KeyCheck::Unknown);
    }

    #[test]
    fn roundtrip_via_file() {
        let mut kh = temp_store();
        kh.pin("h1", 22, "SHA256:one").unwrap();
        kh.pin("h2", 22, "SHA256:two").unwrap();
        let reloaded = KnownHosts::load_from(kh.path.clone()).unwrap();
        assert_eq!(reloaded.check("h1", 22, "SHA256:one"), KeyCheck::Known);
        assert_eq!(reloaded.check("h2", 22, "SHA256:two"), KeyCheck::Known);
    }
}
