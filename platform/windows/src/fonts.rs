//! Per-app font management.
//!
//! Fonts live in `%APPDATA%\TermForge\fonts` and are registered with
//! `AddFontResourceExW(FR_PRIVATE)` — visible to this process only, no
//! system install or admin rights needed.  Drop any .ttf/.otf into that
//! folder and it's available at next launch; or download one of the curated
//! fonts from the command palette without leaving the terminal.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{AddFontResourceExW, FR_PRIVATE};

/// Curated downloadable fonts: (display name, GDI family name, file, URL).
pub const FONT_CATALOG: &[(&str, &str, &str, &str)] = &[
    (
        "JetBrains Mono",
        "JetBrains Mono",
        "JetBrainsMono-Regular.ttf",
        "https://github.com/JetBrains/JetBrainsMono/raw/master/fonts/ttf/JetBrainsMono-Regular.ttf",
    ),
    (
        "Fira Code",
        "Fira Code",
        "FiraCode-Regular.ttf",
        "https://github.com/tonsky/FiraCode/raw/master/distr/ttf/FiraCode-Regular.ttf",
    ),
    (
        "Hack",
        "Hack",
        "Hack-Regular.ttf",
        "https://github.com/source-foundry/Hack/raw/master/build/ttf/Hack-Regular.ttf",
    ),
];

pub fn fonts_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    appdata.join("TermForge").join("fonts")
}

fn register_font_file(path: &std::path::Path) -> bool {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let added = unsafe { AddFontResourceExW(PCWSTR(wide.as_ptr()), FR_PRIVATE, None) };
    added > 0
}

use std::os::windows::ffi::OsStrExt;

/// Register every font file in the fonts dir for this process.
/// Call once at startup, before the compositor resolves font families.
pub fn register_private_fonts() -> usize {
    let dir = fonts_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else { return 0 };
    let mut n = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if matches!(ext.as_deref(), Some("ttf") | Some("otf")) && register_font_file(&path) {
            n += 1;
        }
    }
    if n > 0 {
        tracing::info!("registered {n} private font(s) from {}", dir.display());
    }
    n
}

/// Download catalog entry `idx` into the fonts dir (via curl.exe, which
/// ships with Windows 10+) and register it.  Blocking — run on a worker
/// thread.  Returns the GDI family name to apply.
pub fn download_and_register(idx: usize) -> Result<String> {
    let Some(&(name, family, file, url)) = FONT_CATALOG.get(idx) else {
        bail!("unknown font index {idx}");
    };
    let dir = fonts_dir();
    std::fs::create_dir_all(&dir).context("create fonts dir")?;
    let dest = dir.join(file);

    if !dest.exists() {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = std::process::Command::new("curl.exe")
            .args(["-L", "--fail", "--silent", "-o"])
            .arg(&dest)
            .arg(url)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .context("launch curl.exe")?;
        if !status.success() {
            let _ = std::fs::remove_file(&dest);
            bail!("download failed for {name} ({url})");
        }
    }

    if !register_font_file(&dest) {
        bail!("AddFontResourceEx failed for {}", dest.display());
    }
    Ok(family.to_string())
}
