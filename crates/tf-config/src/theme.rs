//! User-defined colour themes.
//!
//! A theme file is a TOML document: the three generator inputs (`base`,
//! `accent`, `contrast`) plus optional per-slot overrides in `[colors]`.
//! Alacritty colour files (`[colors.primary]`, `[colors.normal]`, ...) are
//! accepted too, so existing theme collections load unchanged.
//!
//! Theme files are untrusted input: parsing never panics, rejects malformed
//! colours instead of guessing, and is size-limited when read from disk.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tf_ui::{Rgb, Theme, ThemeInput};

/// Larger files are rejected without being read.
pub const MAX_THEME_BYTES: u64 = 64 * 1024;
/// Files beyond this many in a themes directory are ignored.
pub const MAX_THEME_FILES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    #[error("invalid TOML: {0}")]
    Parse(String),
    #[error("{field}: `{value}` is not a #rrggbb colour")]
    BadColour { field: String, value: String },
    #[error("colors.ansi needs exactly 16 colours, found {0}")]
    AnsiLength(usize),
    #[error("contrast must be between 0 and 100, found {0}")]
    Contrast(u8),
    #[error("{0}")]
    Io(String),
}

macro_rules! color_slots {
    ($($slot:ident),* $(,)?) => {
        /// Per-slot colour overrides, each `#rrggbb`. Unset slots keep the
        /// generated value. Slot names match the semantic slots of the UI.
        #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(default, deny_unknown_fields)]
        pub struct ThemeColors {
            $(
                #[serde(skip_serializing_if = "Option::is_none")]
                pub $slot: Option<String>,
            )*
            /// The 16 ANSI colours, black to bright white.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub ansi: Option<Vec<String>>,
        }

        impl ThemeColors {
            fn validate(&self) -> Result<(), ThemeError> {
                $(
                    if let Some(value) = &self.$slot {
                        check_colour(concat!("colors.", stringify!($slot)), value)?;
                    }
                )*
                if let Some(ansi) = &self.ansi {
                    if ansi.len() != 16 {
                        return Err(ThemeError::AnsiLength(ansi.len()));
                    }
                    for value in ansi {
                        check_colour("colors.ansi", value)?;
                    }
                }
                Ok(())
            }

            /// Apply the overrides. Invalid colours are skipped; `parse`
            /// has already rejected them for specs that came from text.
            fn apply(&self, theme: &mut Theme) {
                $(
                    if let Some(c) = self.$slot.as_deref().and_then(Rgb::from_hex) {
                        theme.$slot = c;
                    }
                )*
                if let Some(ansi) = &self.ansi {
                    if ansi.len() == 16 {
                        for (slot, value) in theme.ansi.iter_mut().zip(ansi) {
                            if let Some(c) = Rgb::from_hex(value) {
                                *slot = c;
                            }
                        }
                    }
                }
            }
        }
    };
}

color_slots!(
    bg_app,
    bg_panel,
    bg_elevated,
    bg_hover,
    bg_selected,
    border,
    border_strong,
    text,
    text_muted,
    text_faint,
    accent,
    accent_text,
    focus_ring,
    success,
    warning,
    danger,
    term_bg,
    term_fg,
    term_cursor,
    term_selection,
);

fn check_colour(field: &str, value: &str) -> Result<Rgb, ThemeError> {
    Rgb::from_hex(value).ok_or_else(|| ThemeError::BadColour {
        field: field.to_owned(),
        value: value.chars().take(32).collect(),
    })
}

/// A theme as written on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeSpec {
    /// Name shown in the picker. Defaults to the file name.
    pub name: String,
    /// Background colour the surfaces are derived from, `#rrggbb`.
    pub base: String,
    /// Accent colour, `#rrggbb`.
    pub accent: String,
    /// 0 = soft, 100 = maximum contrast.
    pub contrast: u8,
    /// Optional per-slot overrides.
    #[serde(skip_serializing_if = "ThemeColors::is_empty")]
    pub colors: ThemeColors,
}

impl ThemeColors {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for ThemeSpec {
    fn default() -> Self {
        Self::from_input("", ThemeInput::DARK)
    }
}

impl ThemeSpec {
    pub fn from_input(name: &str, input: ThemeInput) -> Self {
        Self {
            name: name.to_owned(),
            base: input.base.to_hex(),
            accent: input.accent.to_hex(),
            contrast: input.contrast,
            colors: ThemeColors::default(),
        }
    }

    /// Parse and validate theme text, native or Alacritty format.
    pub fn parse(text: &str) -> Result<Self, ThemeError> {
        let value: toml::Value =
            toml::from_str(text).map_err(|e| ThemeError::Parse(e.to_string()))?;
        let is_alacritty = value
            .get("colors")
            .is_some_and(|colors| colors.get("primary").is_some());
        let spec = if is_alacritty {
            Self::from_alacritty(&value)?
        } else {
            value
                .try_into::<Self>()
                .map_err(|e| ThemeError::Parse(e.to_string()))?
        };
        spec.validate()?;
        Ok(spec)
    }

    fn validate(&self) -> Result<(), ThemeError> {
        check_colour("base", &self.base)?;
        check_colour("accent", &self.accent)?;
        if self.contrast > 100 {
            return Err(ThemeError::Contrast(self.contrast));
        }
        self.colors.validate()
    }

    /// Generator inputs; falls back to the default dark theme's values for
    /// anything unparseable so a hand-built spec can never panic here.
    pub fn input(&self) -> ThemeInput {
        ThemeInput {
            base: Rgb::from_hex(&self.base).unwrap_or(ThemeInput::DARK.base),
            accent: Rgb::from_hex(&self.accent).unwrap_or(ThemeInput::DARK.accent),
            contrast: self.contrast.min(100),
        }
    }

    pub fn build(&self) -> Theme {
        let mut theme = Theme::generate(self.input());
        self.colors.apply(&mut theme);
        theme
    }

    /// Whether the result is a dark theme, without building it twice.
    pub fn is_dark(&self) -> bool {
        self.build().is_dark
    }

    fn from_alacritty(value: &toml::Value) -> Result<Self, ThemeError> {
        let colors = value.get("colors");
        let get = |group: &str, key: &str| -> Result<Option<String>, ThemeError> {
            let field = format!("colors.{group}.{key}");
            match colors
                .and_then(|c| c.get(group))
                .and_then(|g| g.get(key))
                .and_then(toml::Value::as_str)
            {
                Some(raw) => {
                    let raw = raw.trim();
                    let digits = raw
                        .strip_prefix("0x")
                        .or_else(|| raw.strip_prefix('#'))
                        .unwrap_or(raw);
                    Ok(Some(check_colour(&field, digits)?.to_hex()))
                }
                None => Ok(None),
            }
        };
        let background = get("primary", "background")?.ok_or_else(|| ThemeError::BadColour {
            field: "colors.primary.background".into(),
            value: String::new(),
        })?;
        let names = [
            "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        ];
        let mut ansi = Vec::with_capacity(16);
        for group in ["normal", "bright"] {
            for name in names {
                if let Some(c) = get(group, name)? {
                    ansi.push(c);
                }
            }
        }
        let accent = get("normal", "blue")?
            .or(get("cursor", "cursor")?)
            .unwrap_or_else(|| ThemeInput::DARK.accent.to_hex());
        Ok(Self {
            name: String::new(),
            base: background.clone(),
            accent,
            contrast: ThemeInput::DARK.contrast,
            colors: ThemeColors {
                term_bg: Some(background),
                term_fg: get("primary", "foreground")?,
                term_cursor: get("cursor", "cursor")?,
                term_selection: get("selection", "background")?,
                ansi: (ansi.len() == 16).then_some(ansi),
                ..ThemeColors::default()
            },
        })
    }
}

/// Themes that ship with TermForge. They are generated from a base and an
/// accent, so they follow the contrast rules like any other theme.
pub fn builtin_themes() -> Vec<ThemeSpec> {
    let hex = |s: &str| Rgb::from_hex(s).unwrap_or(ThemeInput::DARK.base);
    let make = |name: &str, base: &str, accent: &str, contrast: u8| {
        ThemeSpec::from_input(
            name,
            ThemeInput {
                base: hex(base),
                accent: hex(accent),
                contrast,
            },
        )
    };
    vec![
        ThemeSpec::from_input("Dark", ThemeInput::DARK),
        ThemeSpec::from_input("Light", ThemeInput::LIGHT),
        make("Midnight", "#0b1020", "#7aa2f7", 35),
        make("Graphite", "#1c1c1e", "#0a84ff", 40),
        make("Forest", "#0f1a14", "#4cb782", 30),
        make("Ember", "#1a1210", "#e5834b", 35),
        make("Violet", "#17121f", "#a78bfa", 30),
        make("Solar Dark", "#002b36", "#268bd2", 30),
        make("Solar Light", "#fdf6e3", "#268bd2", 30),
        make("Paper", "#f7f4ee", "#b45309", 40),
    ]
}

/// Result of scanning a themes directory.
#[derive(Debug, Default)]
pub struct LoadedThemes {
    /// Built-in themes followed by user themes; a user theme replaces a
    /// built-in or earlier theme of the same name (case-insensitive).
    pub themes: Vec<ThemeSpec>,
    /// One human-readable line per file that could not be loaded.
    pub errors: Vec<String>,
}

impl LoadedThemes {
    pub fn find(&self, name: &str) -> Option<&ThemeSpec> {
        self.themes
            .iter()
            .find(|theme| theme.name.eq_ignore_ascii_case(name))
    }
}

/// Built-in themes plus every `*.toml` file in `dir`. A missing directory
/// is not an error: most users never create one.
pub fn load_themes(dir: &Path) -> LoadedThemes {
    let mut loaded = LoadedThemes {
        themes: builtin_themes(),
        errors: Vec::new(),
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return loaded;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
        })
        .collect();
    files.sort();
    if files.len() > MAX_THEME_FILES {
        loaded.errors.push(format!(
            "{} has {} theme files; only the first {MAX_THEME_FILES} are read",
            dir.display(),
            files.len()
        ));
        files.truncate(MAX_THEME_FILES);
    }
    for path in files {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match read_theme_file(&path) {
            Ok(spec) => {
                match loaded
                    .themes
                    .iter_mut()
                    .find(|t| t.name.eq_ignore_ascii_case(&spec.name))
                {
                    Some(existing) => *existing = spec,
                    None => loaded.themes.push(spec),
                }
            }
            Err(e) => loaded.errors.push(format!("{file}: {e}")),
        }
    }
    loaded
}

fn read_theme_file(path: &Path) -> Result<ThemeSpec, ThemeError> {
    let io = |e: std::io::Error| ThemeError::Io(e.to_string());
    let len = std::fs::metadata(path).map_err(io)?.len();
    if len > MAX_THEME_BYTES {
        return Err(ThemeError::Io(format!(
            "file is {len} bytes; the limit is {MAX_THEME_BYTES}"
        )));
    }
    let text = std::fs::read_to_string(path).map_err(io)?;
    let mut spec = ThemeSpec::parse(&text)?;
    if spec.name.trim().is_empty() {
        spec.name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn native_theme_applies_overrides() {
        let spec = ThemeSpec::parse(
            r##"
            name = "Test"
            base = "#1e1e2e"
            accent = "#cba6f7"
            [colors]
            term_fg = "#ffffff"
            "##,
        )
        .unwrap();
        let theme = spec.build();
        assert_eq!(theme.term_fg, Rgb::new(255, 255, 255));
        assert_eq!(theme.accent, Rgb::from_hex("#cba6f7").unwrap());
    }

    #[test]
    fn rejects_bad_colours_and_ansi_length() {
        assert!(matches!(
            ThemeSpec::parse("base = \"red\""),
            Err(ThemeError::BadColour { .. })
        ));
        assert_eq!(
            ThemeSpec::parse("[colors]\nansi = [\"#000000\"]"),
            Err(ThemeError::AnsiLength(1))
        );
        assert_eq!(
            ThemeSpec::parse("contrast = 200"),
            Err(ThemeError::Contrast(200))
        );
        assert!(ThemeSpec::parse("unknown = 1").is_err());
    }

    #[test]
    fn alacritty_files_are_imported() {
        let text = r##"
            [colors.primary]
            background = "0x282a36"
            foreground = "#f8f8f2"
            [colors.normal]
            black = "0x000000"
            red = "0xff5555"
            green = "0x50fa7b"
            yellow = "0xf1fa8c"
            blue = "0xbd93f9"
            magenta = "0xff79c6"
            cyan = "0x8be9fd"
            white = "0xbfbfbf"
            [colors.bright]
            black = "0x4d4d4d"
            red = "0xff6e67"
            green = "0x5af78e"
            yellow = "0xf4f99d"
            blue = "0xcaa9fa"
            magenta = "0xff92d0"
            cyan = "0x9aedfe"
            white = "0xe6e6e6"
        "##;
        let spec = ThemeSpec::parse(text).unwrap();
        let theme = spec.build();
        assert_eq!(theme.term_bg, Rgb::from_hex("#282a36").unwrap());
        assert_eq!(theme.term_fg, Rgb::from_hex("#f8f8f2").unwrap());
        assert_eq!(theme.ansi[1], Rgb::from_hex("#ff5555").unwrap());
        assert_eq!(theme.ansi[15], Rgb::from_hex("#e6e6e6").unwrap());
    }

    #[test]
    fn builtin_themes_meet_contrast_targets() {
        for spec in builtin_themes() {
            let issues = spec.build().contrast_issues();
            assert!(issues.is_empty(), "{}: {issues:?}", spec.name);
        }
    }

    #[test]
    fn load_dir_reports_bad_files_and_replaces_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mine.toml"),
            "name = \"Dark\"\nbase = \"#000000\"\naccent = \"#ff0000\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("broken.toml"), "base = 5").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "x").unwrap();
        let loaded = load_themes(dir.path());
        assert_eq!(loaded.errors.len(), 1, "{:?}", loaded.errors);
        assert!(loaded.errors[0].starts_with("broken.toml"));
        assert_eq!(loaded.themes.len(), builtin_themes().len());
        assert_eq!(loaded.find("dark").unwrap().base, "#000000");
    }

    #[test]
    fn missing_dir_yields_builtins_only() {
        let loaded = load_themes(Path::new("/nonexistent/termforge/themes"));
        assert!(loaded.errors.is_empty());
        assert_eq!(loaded.themes.len(), builtin_themes().len());
    }

    proptest! {
        #[test]
        fn parse_never_panics(text in "\\PC{0,400}") {
            let _ = ThemeSpec::parse(&text);
        }

        #[test]
        fn accepted_specs_build_and_round_trip(
            base in "[0-9a-f]{6}",
            accent in "[0-9a-f]{6}",
            contrast in 0u8..=100,
        ) {
            let text = format!("base = \"#{base}\"\naccent = \"#{accent}\"\ncontrast = {contrast}\n");
            let spec = ThemeSpec::parse(&text).unwrap();
            let _ = spec.build();
            let written = toml::to_string_pretty(&spec).unwrap();
            prop_assert_eq!(ThemeSpec::parse(&written).unwrap(), spec);
        }
    }
}
