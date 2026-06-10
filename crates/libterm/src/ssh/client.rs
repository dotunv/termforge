//! SSH client backed by `russh`.
//!
//! [`SshClient::connect`] is an async function that establishes an SSH
//! connection, authenticates, opens a PTY+shell channel, and returns an
//! [`SshPty`] that implements the [`Pty`] trait — the same interface as
//! [`crate::pty::conpty::ConPty`].
//!
//! Data flows:
//!   PTY output:  channel → tokio task → output_tx → VtParser (main loop)
//!   PTY input:   main loop → SshPty::write → cmd_tx → tokio task → channel

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tokio::sync::mpsc::{self, UnboundedSender};

use crate::pty::Pty;
use crate::ssh::known_hosts::{KeyCheck, KnownHosts};

// ── Channel command sent from SshPty to the I/O task ─────────────────────────

enum SshCmd {
    Data(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

// ── Authentication method ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SshAuth {
    Password(String),
    PrivateKey {
        key_path: PathBuf,
        passphrase: Option<String>,
    },
    /// Try every key in `~/.ssh/` with no passphrase (Phase 4 quick-connect).
    Agent,
}

// ── russh handler — TOFU host-key verification ────────────────────────────────

struct HostKeyVerifier {
    host: String,
    port: u16,
}

#[async_trait]
impl russh::client::Handler for HostKeyVerifier {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = format!("SHA256:{}", server_public_key.fingerprint());
        let mut store = match KnownHosts::load() {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("known_hosts load failed: {e}");
                return Ok(false);
            }
        };
        match store.check(&self.host, self.port, &fingerprint) {
            KeyCheck::Known => Ok(true),
            KeyCheck::Unknown => {
                // First connection: pin the key (trust-on-first-use).
                tracing::info!(
                    "pinning new host key for {}:{} ({fingerprint})",
                    self.host, self.port
                );
                if let Err(e) = store.pin(&self.host, self.port, &fingerprint) {
                    tracing::error!("known_hosts pin failed: {e}");
                    return Ok(false);
                }
                Ok(true)
            }
            KeyCheck::Mismatch => {
                tracing::error!(
                    "HOST KEY MISMATCH for {}:{} — got {fingerprint}. \
                     Possible man-in-the-middle attack. If the server key \
                     legitimately changed, remove the entry from known_hosts.",
                    self.host, self.port
                );
                Ok(false)
            }
        }
    }
}

// ── SshPty — implements Pty for SSH sessions ──────────────────────────────────

pub struct SshPty {
    cmd_tx: mpsc::UnboundedSender<SshCmd>,
}

impl Pty for SshPty {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.cmd_tx
            .send(SshCmd::Data(data.to_vec()))
            .map_err(|_| anyhow::anyhow!("SSH channel closed"))
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.cmd_tx
            .send(SshCmd::Resize { cols, rows })
            .map_err(|_| anyhow::anyhow!("SSH channel closed"))
    }
}

// ── Public connection entry point ─────────────────────────────────────────────

pub struct SshClient;

impl SshClient {
    /// Connect to `host:port`, authenticate, open a PTY+shell channel.
    /// Returns a [`SshPty`] handle.  Must be called inside a tokio runtime.
    pub async fn connect(
        host: &str,
        port: u16,
        username: &str,
        auth: SshAuth,
        cols: u16,
        rows: u16,
        output_tx: UnboundedSender<Vec<u8>>,
    ) -> Result<SshPty> {
        let config = Arc::new(russh::client::Config::default());
        let handler = HostKeyVerifier {
            host: host.to_string(),
            port,
        };

        let mut session = russh::client::connect(config, (host, port), handler)
            .await
            .context("SSH TCP connect")?;

        // Authenticate
        let authed = match auth {
            SshAuth::Password(ref pw) => session
                .authenticate_password(username, pw)
                .await
                .context("SSH password auth")?,

            SshAuth::PrivateKey {
                ref key_path,
                ref passphrase,
            } => {
                let key = russh_keys::load_secret_key(key_path, passphrase.as_deref())
                    .context("load private key")?;
                session
                    .authenticate_publickey(username, Arc::new(key))
                    .await
                    .context("SSH pubkey auth")?
            }

            SshAuth::Agent => {
                // Try keys from ~/.ssh/ with no passphrase
                let ssh_dir = dirs_key_path();
                let mut ok = false;
                for path in ssh_private_keys(&ssh_dir) {
                    if let Ok(key) = russh_keys::load_secret_key(&path, None) {
                        if let Ok(true) = session
                            .authenticate_publickey(username, Arc::new(key))
                            .await
                        {
                            ok = true;
                            break;
                        }
                    }
                }
                ok
            }
        };

        if !authed {
            bail!("SSH authentication failed for {username}@{host}");
        }

        // Open a session channel
        let mut channel = session
            .channel_open_session()
            .await
            .context("channel_open_session")?;

        // Request a PTY
        channel
            .request_pty(
                false,
                "xterm-256color",
                cols as u32,
                rows as u32,
                0,
                0,
                &[], // terminal modes
            )
            .await
            .context("request_pty")?;

        // Start a shell
        channel
            .request_shell(false)
            .await
            .context("request_shell")?;

        // Spawn the async I/O loop
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SshCmd>();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Incoming bytes from the remote shell
                    msg = channel.wait() => {
                        match msg {
                            Some(russh::ChannelMsg::Data { data }) => {
                                let _ = output_tx.send(data.to_vec());
                            }
                            Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                                // stderr — also forward to terminal
                                let _ = output_tx.send(data.to_vec());
                            }
                            None | Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) => {
                                break;
                            }
                            _ => {}
                        }
                    }

                    // Commands from the main thread
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(SshCmd::Data(bytes)) => {
                                let _ = channel.data(bytes.as_ref()).await;
                            }
                            Some(SshCmd::Resize { cols, rows }) => {
                                let _ = channel
                                    .window_change(cols as u32, rows as u32, 0, 0)
                                    .await;
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        Ok(SshPty { cmd_tx })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn dirs_key_path() -> PathBuf {
    let mut p = dirs_home();
    p.push(".ssh");
    p
}

fn dirs_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns paths to likely private key files in `dir`.
fn ssh_private_keys(dir: &Path) -> Vec<PathBuf> {
    let candidates = ["id_ed25519", "id_rsa", "id_ecdsa", "id_dsa"];
    candidates
        .iter()
        .map(|n| dir.join(n))
        .filter(|p| p.exists())
        .collect()
}
