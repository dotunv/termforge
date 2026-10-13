//! Monospace font selection.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{px, App, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, SharedString};

/// Preferred terminal fonts, best first. Cascadia ships with Windows
/// Terminal and Windows 11; Consolas ships with every supported Windows.
const PREFERRED: &[&str] = &[
    "Cascadia Mono",
    "Cascadia Code",
    "JetBrains Mono",
    "Consolas",
    "SF Mono",
    "Menlo",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
];

/// Monospace-capable fallbacks ordered by the content they commonly cover.
/// Only installed families are included: asking GPUI to resolve a missing
/// fallback can make the entire font lookup fail on some platforms.
const PREFERRED_FALLBACKS: &[&str] = &[
    "Symbols Nerd Font Mono",
    "Noto Sans Mono CJK SC",
    "Noto Sans Mono CJK JP",
    "Noto Sans Mono CJK KR",
    "Noto Sans Mono",
    "Cascadia Mono",
    "Segoe UI Symbol",
    "Apple Symbols",
    "DejaVu Sans Mono",
];

#[derive(Debug, Clone)]
pub struct MonoFont {
    pub family: SharedString,
    fallbacks: Vec<String>,
    ligatures: bool,
    /// The configured family was not installed, so detection chose instead.
    pub requested_missing: bool,
}

impl MonoFont {
    /// Use `preferred` when it is installed, otherwise the best detected
    /// monospace face. An empty `preferred` means "detect".
    pub fn new(cx: &App, preferred: &str, ligatures: bool) -> Self {
        let available = cx.text_system().all_font_names();
        let installed = |want: &str| {
            available
                .iter()
                .find(|have| have.eq_ignore_ascii_case(want))
                .cloned()
        };
        let requested = preferred.trim();
        let chosen = (!requested.is_empty())
            .then(|| installed(requested))
            .flatten();
        let family = chosen.clone().unwrap_or_else(|| {
            PREFERRED
                .iter()
                .find_map(|want| installed(want))
                .unwrap_or_else(|| "monospace".to_owned())
        });
        let fallbacks = fallback_families(&available, &family);
        tracing::info!(font = %family, ?fallbacks, "terminal font");
        Self {
            family: family.into(),
            fallbacks,
            ligatures,
            requested_missing: !requested.is_empty() && chosen.is_none(),
        }
    }

    pub fn regular(&self) -> Font {
        Font {
            family: self.family.clone(),
            // Terminals want fixed-width digits and no ligatures by default.
            features: if self.ligatures {
                FontFeatures::default()
            } else {
                FontFeatures::disable_ligatures()
            },
            fallbacks: (!self.fallbacks.is_empty())
                .then(|| FontFallbacks::from_fonts(self.fallbacks.clone())),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }

    pub fn fallback_names(&self) -> &[String] {
        &self.fallbacks
    }
}

fn fallback_families(available: &[String], primary: &str) -> Vec<String> {
    PREFERRED_FALLBACKS
        .iter()
        .filter(|candidate| !candidate.eq_ignore_ascii_case(primary))
        .filter_map(|candidate| {
            available
                .iter()
                .find(|installed| installed.eq_ignore_ascii_case(candidate))
                .cloned()
        })
        .collect()
}

/// Proportional fonts for interface chrome, best first. The terminal grid
/// keeps its monospace font; labels, hints and settings read better in a
/// proportional face and do not run together at small sizes.
const PREFERRED_UI: &[&str] = &[
    "Inter",
    "Segoe UI Variable",
    "Segoe UI",
    "SF Pro Text",
    "Helvetica Neue",
    "Cantarell",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "Arial",
];

/// The family used for interface text: `preferred` when installed, else the
/// best proportional face, else the terminal font.
pub fn ui_family(cx: &App, preferred: &str, fallback: &SharedString) -> SharedString {
    let available = cx.text_system().all_font_names();
    let installed = |want: &str| {
        available
            .iter()
            .find(|have| have.eq_ignore_ascii_case(want))
            .cloned()
    };
    let requested = preferred.trim();
    let family = (!requested.is_empty())
        .then(|| installed(requested))
        .flatten()
        .or_else(|| PREFERRED_UI.iter().find_map(|want| installed(want)))
        .map(SharedString::from)
        .unwrap_or_else(|| fallback.clone());
    tracing::info!(font = %family, "interface font");
    family
}

/// Installed families that look monospaced: `i` and `m` advance the same
/// distance. GPUI exposes no "is monospace" flag, so measure instead.
pub fn monospace_families(cx: &App) -> Vec<String> {
    let text_system = cx.text_system();
    text_system
        .all_font_names()
        .into_iter()
        .filter(|name| !name.starts_with('.'))
        .filter(|name| {
            let font = Font {
                family: name.clone().into(),
                features: FontFeatures::default(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            };
            let id = text_system.resolve_font(&font);
            let advance = |ch| text_system.advance(id, px(16.0), ch).map(|s| s.width);
            matches!((advance('i'), advance('m')), (Ok(i), Ok(m)) if i > px(0.0) && i == m)
        })
        .collect()
}

/// Installed families usable for interface text.
pub fn all_families(cx: &App) -> Vec<String> {
    cx.text_system()
        .all_font_names()
        .into_iter()
        .filter(|name| !name.starts_with('.'))
        .collect()
}

/// Largest font file read from disk.
const MAX_FONT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FONT_FILES: usize = 128;

/// What a pass over the user fonts folder did.
#[derive(Debug, Default, Clone)]
pub struct FontLoadReport {
    /// File names registered with the text system.
    pub loaded: Vec<String>,
    /// One line per file that was skipped or rejected.
    pub errors: Vec<String>,
}

/// Register every font file in `dir` that is not in `already`. Each file is
/// added on its own so one corrupt font cannot hide the rest. The text
/// system cannot unregister fonts, so a font changed on disk needs a restart.
pub fn load_user_fonts(cx: &App, dir: &Path, already: &mut HashSet<PathBuf>) -> FontLoadReport {
    let mut report = FontLoadReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return report;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| {
                ["ttf", "otf", "ttc"]
                    .iter()
                    .any(|want| ext.eq_ignore_ascii_case(want))
            })
        })
        .collect();
    files.sort();
    if files.len() > MAX_FONT_FILES {
        report.errors.push(format!(
            "{} has {} font files; only the first {MAX_FONT_FILES} are read",
            dir.display(),
            files.len()
        ));
        files.truncate(MAX_FONT_FILES);
    }
    for path in files {
        if already.contains(&path) {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_FONT_BYTES => {
                report.errors.push(format!("{name}: larger than 32 MiB"));
                continue;
            }
            Ok(_) => match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    report.errors.push(format!("{name}: {e}"));
                    continue;
                }
            },
            Err(e) => {
                report.errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        match cx.text_system().add_fonts(vec![Cow::Owned(bytes)]) {
            Ok(()) => {
                already.insert(path);
                report.loaded.push(name);
            }
            Err(e) => report.errors.push(format!("{name}: {e}")),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallbacks_are_installed_ordered_and_exclude_primary() {
        let installed = vec![
            "Noto Sans Mono".to_owned(),
            "Cascadia Mono".to_owned(),
            "Symbols Nerd Font Mono".to_owned(),
            "Unrelated".to_owned(),
        ];
        assert_eq!(
            fallback_families(&installed, "Cascadia Mono"),
            vec!["Symbols Nerd Font Mono", "Noto Sans Mono"]
        );
    }
}
