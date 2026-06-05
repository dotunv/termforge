//! Persistent SSH host configuration store.
//!
//! Hosts are stored in TOML at `%APPDATA%\TermForge\hosts.toml` on Windows,
//! `~/.config/termforge/hosts.toml` on other platforms.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostConfig {
    /// Display name shown in the SSH manager.
    pub name: String,
    /// Hostname or IP address.
    pub hostname: String,
    /// TCP port (default 22).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Remote username.
    pub username: String,
    /// Path to the private key file. `None` = try agent / default keys.
    pub key_path: Option<String>,
    /// Optional grouping label (e.g. "production", "staging").
    pub group: Option<String>,
    /// Optional jump host name (must be another entry in the store).
    pub jump_host: Option<String>,
}

fn default_port() -> u16 { 22 }

impl HostConfig {
    pub fn new(name: impl Into<String>, hostname: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            hostname: hostname.into(),
            port: 22,
            username: username.into(),
            key_path: None,
            group: None,
            jump_host: None,
        }
    }

    pub fn display_label(&self) -> String {
        format!("{}@{}:{}", self.username, self.hostname, self.port)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    hosts: Vec<HostConfig>,
}

/// Manages the collection of saved SSH hosts.
pub struct HostStore {
    pub hosts: Vec<HostConfig>,
    pub path: PathBuf,
}

impl HostStore {
    /// Load from disk (creates an empty store if the file doesn't exist).
    pub fn load() -> Result<Self> {
        let path = store_path();
        let hosts = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .context("read hosts.toml")?;
            let file: StoreFile = toml::from_str(&text)
                .context("parse hosts.toml")?;
            file.hosts
        } else {
            Vec::new()
        };
        Ok(Self { hosts, path })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).context("create config dir")?;
        }
        let file = StoreFile { hosts: self.hosts.clone() };
        let text = toml::to_string_pretty(&file).context("serialise hosts")?;
        std::fs::write(&self.path, text).context("write hosts.toml")?;
        Ok(())
    }

    pub fn add(&mut self, host: HostConfig) {
        self.hosts.push(host);
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.hosts.len() {
            self.hosts.remove(index);
        }
    }

    pub fn get(&self, index: usize) -> Option<&HostConfig> {
        self.hosts.get(index)
    }
}

impl Default for HostStore {
    fn default() -> Self {
        Self { hosts: Vec::new(), path: store_path() }
    }
}

fn store_path() -> PathBuf {
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        appdata.join("TermForge").join("hosts.toml")
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".config").join("termforge").join("hosts.toml")
    }
}
