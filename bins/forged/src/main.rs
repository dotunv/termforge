//! `forged` owns PTY sessions and the local store so terminals survive UI
//! restarts. Phase 0 ships the skeleton: configuration, storage and
//! diagnostics. The named-pipe server lands in Phase 2
//! (`docs/adr/0005-process-model.md`, `docs/adr/0006-ipc-security.md`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "forged", version, about = "TermForge session host")]
struct Cli {
    /// Override the data directory.
    #[arg(long, env = "TERMFORGE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the session host (default).
    Run,
    /// Print environment diagnostics.
    Doctor,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TERMFORGE_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let data_dir = match cli.data_dir {
        Some(d) => d,
        None => directories::ProjectDirs::from("dev", "TermForge", "TermForge")
            .context("could not resolve a data directory")?
            .data_local_dir()
            .to_path_buf(),
    };
    std::fs::create_dir_all(&data_dir).with_context(|| format!("create {}", data_dir.display()))?;

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(&data_dir),
        Command::Doctor => doctor(&data_dir),
    }
}

fn run(data_dir: &std::path::Path) -> Result<()> {
    let store = tf_store::Store::open(&data_dir.join("termforge.db"))?;
    tf_shell::install(&data_dir.join("shell"))?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        protocol = tf_proto::PROTOCOL_VERSION,
        schema = store.schema_version()?,
        data = %data_dir.display(),
        "forged started"
    );
    tracing::warn!("IPC server is not implemented yet (Phase 2); exiting");
    Ok(())
}

#[allow(clippy::print_stdout)]
fn doctor(data_dir: &std::path::Path) -> Result<()> {
    let store = tf_store::Store::open(&data_dir.join("termforge.db"))?;
    println!("forged {}", env!("CARGO_PKG_VERSION"));
    println!("protocol     v{}", tf_proto::PROTOCOL_VERSION);
    println!("data dir     {}", data_dir.display());
    println!("schema       v{}", store.schema_version()?);
    let exe_dir = std::env::current_exe()?.parent().map(|p| p.to_path_buf());
    let sideloaded = exe_dir
        .map(|d| d.join("conpty.dll").is_file() && d.join("OpenConsole.exe").is_file())
        .unwrap_or(false);
    if cfg!(windows) {
        println!(
            "conpty       {}",
            if sideloaded {
                "bundled"
            } else {
                "in-box (run `cargo xtask conpty`)"
            }
        );
    }
    println!("shells:");
    for s in tf_pty::discover_shells() {
        println!("  {:<20} {}", s.name, s.program.display());
    }
    Ok(())
}
