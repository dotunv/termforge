//! Appearance: persisted settings, user themes and user fonts.
//!
//! The view owns the live theme and fonts; this owns what is on disk and
//! how it resolves. Everything here degrades to defaults rather than
//! failing, and never overwrites a config file the user broke by hand.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use gpui::App;
use tf_config::theme::{load_themes, LoadedThemes, ThemeSpec};
use tf_config::{Config, ThemeMode};
use tf_ui::Theme;

use crate::fonts::{load_user_fonts, monospace_families, FontLoadReport};

#[derive(Debug)]
pub struct Appearance {
    pub config: Config,
    config_path: Option<PathBuf>,
    pub themes_dir: Option<PathBuf>,
    pub fonts_dir: Option<PathBuf>,
    pub themes: LoadedThemes,
    pub fonts: FontLoadReport,
    loaded_fonts: HashSet<PathBuf>,
    /// The config file exists but did not parse. Saving is disabled so the
    /// user's file is not replaced with defaults.
    pub config_error: Option<String>,
    pub save_error: Option<String>,
    mono_families: Option<Vec<String>>,
    /// Hash of the watched files at the last load or save.
    fingerprint: u64,
}

impl Appearance {
    pub fn load(cx: &App) -> Self {
        let mut this = Self {
            config: Config::default(),
            config_path: Config::config_path().ok(),
            themes_dir: Config::themes_dir().ok(),
            fonts_dir: Config::fonts_dir().ok(),
            themes: LoadedThemes::default(),
            fonts: FontLoadReport::default(),
            loaded_fonts: HashSet::new(),
            config_error: None,
            save_error: None,
            mono_families: None,
            fingerprint: 0,
        };
        this.reload(cx);
        this
    }

    /// Built-in themes and defaults only; touches nothing on disk.
    #[cfg(test)]
    pub fn builtin() -> Self {
        Self {
            config: Config::default(),
            config_path: None,
            themes_dir: None,
            fonts_dir: None,
            themes: LoadedThemes {
                themes: tf_config::theme::builtin_themes(),
                errors: Vec::new(),
            },
            fonts: FontLoadReport::default(),
            loaded_fonts: HashSet::new(),
            config_error: None,
            save_error: None,
            mono_families: None,
            fingerprint: 0,
        }
    }

    /// Re-read the config file, the themes folder and the fonts folder.
    pub fn reload(&mut self, cx: &App) {
        match &self.config_path {
            Some(path) if path.exists() => match Config::load_sync(path) {
                Ok(config) => {
                    self.config = config;
                    self.config_error = None;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), "config not applied: {e}");
                    self.config_error = Some(e.to_string());
                }
            },
            _ => self.config_error = None,
        }
        if let Some(dir) = &self.fonts_dir {
            let report = load_user_fonts(cx, dir, &mut self.loaded_fonts);
            // Keep earlier loads in the count; only new errors replace old.
            self.fonts.loaded.extend(report.loaded);
            self.fonts.errors = report.errors;
            self.mono_families = None;
        }
        self.themes = match &self.themes_dir {
            Some(dir) => load_themes(dir),
            None => LoadedThemes {
                themes: tf_config::theme::builtin_themes(),
                errors: Vec::new(),
            },
        };
        self.fingerprint = self.fingerprint_now();
    }

    /// Persist the config. A no-op while the file on disk is unparseable.
    pub fn save(&mut self) {
        self.save_error = None;
        if self.config_error.is_some() {
            return;
        }
        let Some(path) = &self.config_path else {
            return;
        };
        if let Err(e) = self.config.save_sync(path) {
            tracing::warn!(path = %path.display(), "config not saved: {e}");
            self.save_error = Some(e.to_string());
        }
        // Our own write is not an outside change.
        self.fingerprint = self.fingerprint_now();
    }

    /// Cheap stat-based hash of the config file and the theme and font
    /// folders, so edits made elsewhere can be noticed by polling.
    fn fingerprint_now(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        let mut add = |path: &Path| {
            if let Ok(meta) = std::fs::metadata(path) {
                path.hash(&mut hasher);
                meta.len().hash(&mut hasher);
                meta.modified().ok().hash(&mut hasher);
            }
        };
        if let Some(path) = &self.config_path {
            add(path);
        }
        for dir in [&self.themes_dir, &self.fonts_dir].into_iter().flatten() {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> =
                entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            paths.sort();
            for path in paths.iter().take(512) {
                add(path);
            }
        }
        hasher.finish()
    }

    /// True once if the watched files changed since the last load or save.
    pub fn changed_on_disk(&mut self) -> bool {
        let now = self.fingerprint_now();
        let changed = now != self.fingerprint;
        self.fingerprint = now;
        changed
    }

    /// Write the current theme, with accent and contrast overrides baked
    /// in, to the themes folder as `<name> (custom).toml`. Returns the path.
    pub fn export_theme(&self) -> Result<PathBuf, String> {
        let dir = self.themes_dir.as_ref().ok_or("no themes folder")?;
        let mut spec = self.theme_spec();
        spec.name = format!("{} (custom)", spec.name.trim_end_matches(" (custom)"));
        let file: String = spec
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{file}.toml"));
        let text = toml::to_string_pretty(&spec).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// Name of the theme in effect: the configured one when it exists,
    /// otherwise the Dark or Light default for the configured mode.
    pub fn theme_name(&self) -> String {
        let configured = self.config.ui.theme_name.trim();
        if let Some(found) = (!configured.is_empty())
            .then(|| self.themes.find(configured))
            .flatten()
        {
            return found.name.clone();
        }
        match self.config.ui.theme {
            ThemeMode::Light => "Light",
            ThemeMode::Dark | ThemeMode::System => "Dark",
        }
        .to_owned()
    }

    /// The selected theme with the accent and contrast overrides applied.
    pub fn theme_spec(&self) -> ThemeSpec {
        self.resolve(&self.theme_name())
    }

    pub fn theme(&self) -> Theme {
        self.theme_spec().build()
    }

    /// Theme `name` with the current overrides, for live previews.
    pub fn theme_named(&self, name: &str) -> Theme {
        self.resolve(name).build()
    }

    fn resolve(&self, name: &str) -> ThemeSpec {
        let mut spec = self.themes.find(name).cloned().unwrap_or_default();
        if !self.config.ui.accent.is_empty() {
            spec.accent = self.config.ui.accent.clone();
        }
        if let Some(contrast) = self.config.ui.contrast {
            spec.contrast = contrast;
        }
        spec
    }

    /// Monospace families, measured once and cached until fonts reload.
    pub fn mono_families(&mut self, cx: &App) -> Vec<String> {
        self.mono_families
            .get_or_insert_with(|| monospace_families(cx))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_dir(dir: &Path) -> Appearance {
        let mut a = Appearance::builtin();
        a.themes_dir = Some(dir.join("themes"));
        a.config_path = Some(dir.join("config.toml"));
        a
    }

    #[test]
    fn exported_theme_reloads_as_a_custom_theme() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = in_dir(dir.path());
        a.config.ui.theme_name = "Forest".into();
        a.config.ui.accent = "#ff8800".into();
        let path = a.export_theme().unwrap();
        let loaded = tf_config::theme::load_themes(path.parent().unwrap());
        let custom = loaded
            .find("Forest (custom)")
            .expect("exported theme loads");
        assert_eq!(custom.accent, "#ff8800");
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    }

    #[test]
    fn outside_edits_are_noticed_once_and_own_saves_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = in_dir(dir.path());
        a.save();
        assert!(!a.changed_on_disk(), "own save is not an outside change");
        std::fs::create_dir_all(dir.path().join("themes")).unwrap();
        std::fs::write(
            dir.path().join("themes").join("x.toml"),
            "base = \"#000000\"\n",
        )
        .unwrap();
        assert!(a.changed_on_disk());
        assert!(!a.changed_on_disk(), "reported once");
    }
}
