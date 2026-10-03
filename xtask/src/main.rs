//! `cargo xtask <task>`
//!
//! - `conpty [--arch x64|arm64|x86]`: fetch the pinned ConPTY package and
//!   place `conpty.dll` + `OpenConsole.exe` in `assets/conpty/`, verified by
//!   SHA-256.
//! - `ci`: run the same checks CI runs (fmt, clippy, tests).
//! - `spike-a [--commands N] [--shells a,b]`: the long ConPTY ordering
//!   harness from the build plan. Too slow for `ci`, so it is opt-in.
//! - `latency`: keystroke-to-visible-output probe against a real shell,
//!   enforcing the plan's p50 budget. Release only.
#![allow(clippy::print_stdout)]

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};

const CONPTY_VERSION: &str = "1.24.260710001";
const CONPTY_SHA256: &str = "175640566a3b59c4b132070ee96c2c77e5ab7edd2e92732a5eb3610bbf63d90e";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("conpty") => {
            let arch = match args.iter().position(|a| a == "--arch") {
                Some(i) => args.get(i + 1).context("--arch needs a value")?.as_str(),
                None => "x64",
            };
            conpty(arch)
        }
        Some("ci") => ci(),
        Some("latency") => latency(),
        Some("spike-a") => {
            // Passed through to the test rather than parsed here, so the
            // harness keeps its own defaults when run directly.
            let flag = |name: &str| -> Option<String> {
                let i = args.iter().position(|a| a == name)?;
                args.get(i + 1).cloned()
            };
            for (name, var) in [
                ("--commands", "TF_SPIKE_COMMANDS"),
                ("--shells", "TF_SPIKE_SHELLS"),
            ] {
                if let Some(v) = flag(name) {
                    // Single-threaded before any threads are spawned.
                    std::env::set_var(var, v);
                }
            }
            spike_a()
        }
        _ => {
            println!(
                "usage: cargo xtask <conpty [--arch x64|arm64|x86] | ci | \
                 spike-a [--commands N] [--shells list] | latency>"
            );
            Ok(())
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn conpty(arch: &str) -> Result<()> {
    ensure!(
        matches!(arch, "x64" | "arm64" | "x86"),
        "unsupported arch {arch}"
    );
    let out_dir = root().join("assets").join("conpty");
    std::fs::create_dir_all(&out_dir)?;
    let pkg = std::env::temp_dir().join(format!("conpty-{CONPTY_VERSION}.nupkg"));

    if !pkg.is_file() || sha256_file(&pkg)? != CONPTY_SHA256 {
        let url = format!(
            "https://www.nuget.org/api/v2/package/Microsoft.Windows.Console.ConPTY/{CONPTY_VERSION}"
        );
        println!("downloading {url}");
        run(Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&pkg)
            .arg(&url))?;
    }
    let got = sha256_file(&pkg)?;
    if got != CONPTY_SHA256 {
        let _ = std::fs::remove_file(&pkg);
        bail!("ConPTY package hash mismatch: expected {CONPTY_SHA256}, got {got}");
    }

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&pkg)?)?;
    for (entry, name) in [
        (
            format!("runtimes/win-{arch}/native/conpty.dll"),
            "conpty.dll",
        ),
        (
            format!("build/native/runtimes/{arch}/OpenConsole.exe"),
            "OpenConsole.exe",
        ),
    ] {
        let mut file = zip
            .by_name(&entry)
            .with_context(|| format!("{entry} not in package"))?;
        let mut out = std::fs::File::create(out_dir.join(name))?;
        std::io::copy(&mut file, &mut out)?;
        println!("wrote assets/conpty/{name}");
    }
    println!("ConPTY {CONPTY_VERSION} ({arch}) ready. Both files must ship next to the exe.");
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn ci() -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let root = root();
    let step = |args: &[&str]| -> Result<()> {
        println!("\n==> cargo {}", args.join(" "));
        run(Command::new(&cargo).args(args).current_dir(&root))
    };
    step(&["fmt", "--all", "--check"])?;
    step(&[
        "clippy",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ])?;
    step(&["test", "--locked"])?;
    Ok(())
}

/// Keystroke-to-visible-output latency probe. Always release: the budget is
/// meaningless in a debug build of this dependency graph.
fn latency() -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    println!("\n==> cargo test -p tf-session --test latency --release -- --ignored --nocapture");
    run(Command::new(&cargo).current_dir(root()).args([
        "test",
        "-p",
        "tf-session",
        "--test",
        "latency",
        "--release",
        "--locked",
        "--",
        "--ignored",
        "--nocapture",
    ]))
}

/// The Spike A ordering harness: real shell, real PTY, real integration
/// script, many commands. Deliberately not part of `ci`; see the test's docs.
fn spike_a() -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    println!("\n==> cargo test -p tf-session --test spike_a -- --ignored --nocapture");
    run(Command::new(&cargo).current_dir(root()).args([
        "test",
        "-p",
        "tf-session",
        "--test",
        "spike_a",
        "--locked",
        "--",
        "--ignored",
        "--nocapture",
    ]))
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("failed to start {cmd:?}"))?;
    ensure!(status.success(), "{cmd:?} failed with {status}");
    Ok(())
}
