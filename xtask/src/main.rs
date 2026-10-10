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
//! - `dist`: build and stage a self-contained beta bundle for the host OS.
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
        Some("dist") => dist(),
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
                "usage: cargo xtask <conpty [--arch x64|arm64|x86] | ci | dist | \
                 spike-a [--commands N] [--shells list] | latency>"
            );
            Ok(())
        }
    }
}

fn dist() -> Result<()> {
    let root = root();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    if cfg!(windows) {
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "x86",
            arch => bail!("ConPTY is not packaged for Windows architecture {arch}"),
        };
        conpty(arch)?;
    }
    run(Command::new(&cargo).current_dir(&root).args([
        "build",
        "--release",
        "--locked",
        "-p",
        "termforge",
        "-p",
        "forged",
        "-p",
        "tf",
    ]))?;

    let platform = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        os => bail!("release bundles are not defined for {os}"),
    };
    let bundle_name = format!(
        "termforge-{}-{platform}-{}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH
    );
    let bundle = root.join("target").join("dist").join(&bundle_name);
    if bundle.exists() {
        std::fs::remove_dir_all(&bundle)
            .with_context(|| format!("remove stale {}", bundle.display()))?;
    }

    let executable_dir = if cfg!(target_os = "macos") {
        let contents = bundle.join("TermForge.app").join("Contents");
        let macos = contents.join("MacOS");
        let resources = contents.join("Resources");
        std::fs::create_dir_all(&macos)?;
        std::fs::create_dir_all(&resources)?;
        build_macos_icon(&root, &resources.join("TermForge.icns"))?;
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>termforge</string>
<key>CFBundleIdentifier</key><string>dev.termforge.TermForge</string>
<key>CFBundleName</key><string>TermForge</string>
<key>CFBundleIconFile</key><string>TermForge</string>
<key>CFBundleShortVersionString</key><string>{}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
</dict></plist>
"#,
                env!("CARGO_PKG_VERSION")
            ),
        )?;
        macos
    } else {
        let bin = bundle.join("bin");
        std::fs::create_dir_all(&bin)?;
        bin
    };

    let suffix = if cfg!(windows) { ".exe" } else { "" };
    for name in ["termforge", "forged", "tf"] {
        let file = format!("{name}{suffix}");
        copy_required(
            &root.join("target/release").join(&file),
            &executable_dir.join(file),
        )?;
    }
    if cfg!(windows) {
        for file in ["conpty.dll", "OpenConsole.exe"] {
            copy_required(
                &root.join("assets/conpty").join(file),
                &executable_dir.join(file),
            )?;
        }
    }
    if cfg!(target_os = "linux") {
        let applications = bundle.join("share/applications");
        let icons = bundle.join("share/icons/hicolor/256x256/apps");
        std::fs::create_dir_all(&applications)?;
        std::fs::create_dir_all(&icons)?;
        copy_required(
            &root.join("assets/linux/dev.termforge.TermForge.desktop"),
            &applications.join("dev.termforge.TermForge.desktop"),
        )?;
        copy_required(
            &root.join("assets/icons/termforge-256.png"),
            &icons.join("dev.termforge.TermForge.png"),
        )?;
    }
    for file in ["README.md", "LICENSE-MIT", "LICENSE-APACHE"] {
        copy_required(&root.join(file), &bundle.join(file))?;
    }
    println!("bundle ready: {}", bundle.display());
    Ok(())
}

fn build_macos_icon(root: &Path, destination: &Path) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }

    let iconset = root.join("target/dist/TermForge.iconset");
    if iconset.exists() {
        std::fs::remove_dir_all(&iconset)?;
    }
    std::fs::create_dir_all(&iconset)?;
    let source = root.join("assets/icons/termforge-icon-v3-forge.png");
    for (name, size) in [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ] {
        run(Command::new("sips")
            .args(["-z", &size.to_string(), &size.to_string()])
            .arg(&source)
            .args(["--out"])
            .arg(iconset.join(name)))?;
    }
    run(Command::new("iconutil")
        .args(["-c", "icns"])
        .arg(&iconset)
        .args(["-o"])
        .arg(destination))?;
    std::fs::remove_dir_all(iconset)?;
    Ok(())
}

fn copy_required(source: &Path, destination: &Path) -> Result<()> {
    ensure!(
        source.is_file(),
        "required bundle file missing: {}",
        source.display()
    );
    std::fs::copy(source, destination)
        .with_context(|| format!("copy {} to {}", source.display(), destination.display()))?;
    Ok(())
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
