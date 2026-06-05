//! SSH key discovery and management.
//!
//! Phase 4: discovers keys from `~/.ssh/` and exposes them for selection in
//! the SSH manager.  Passphrase storage via Windows Credential Manager is
//! planned for Phase 4+.

use std::path::PathBuf;

/// A discovered SSH private key.
#[derive(Debug, Clone)]
pub struct SshKey {
    /// Full path to the private key file.
    pub path: PathBuf,
    /// Display name (e.g. "id_ed25519").
    pub name: String,
    /// Key type detected from the file header (best-effort).
    pub kind: KeyKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyKind {
    Ed25519,
    Rsa,
    Ecdsa,
    Unknown,
}

/// Scan `~/.ssh/` for private key files and return metadata for each.
pub fn discover_keys() -> Vec<SshKey> {
    let ssh_dir = ssh_dir();
    let candidates = [
        ("id_ed25519", KeyKind::Ed25519),
        ("id_rsa",     KeyKind::Rsa),
        ("id_ecdsa",   KeyKind::Ecdsa),
    ];

    let mut keys = Vec::new();
    for (filename, kind) in &candidates {
        let path = ssh_dir.join(filename);
        if path.exists() {
            keys.push(SshKey {
                name: filename.to_string(),
                kind: kind.clone(),
                path,
            });
        }
    }

    // Also scan for any other files that look like private keys.
    if let Ok(entries) = std::fs::read_dir(&ssh_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().is_some() { continue; } // skip .pub files
            let name = p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if candidates.iter().any(|(f, _)| *f == name) { continue; }
            if is_private_key(&p) {
                keys.push(SshKey {
                    name,
                    kind: KeyKind::Unknown,
                    path: p,
                });
            }
        }
    }

    keys
}

fn is_private_key(path: &PathBuf) -> bool {
    // Peek at first line to check for PEM/OpenSSH header.
    std::fs::read_to_string(path)
        .ok()
        .map(|s| {
            s.starts_with("-----BEGIN ")
                || s.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----")
        })
        .unwrap_or(false)
}

fn ssh_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".ssh")
}
