use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font: FontConfig,
    pub colors: ColorScheme,
    pub shell: ShellConfig,
    pub window: WindowConfig,
    pub scrollback_lines: usize,
    pub cursor: CursorConfig,
    pub opacity: f32,
    pub padding: PaddingConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font: FontConfig::default(),
            colors: ColorScheme::default(),
            shell: ShellConfig::default(),
            window: WindowConfig::default(),
            scrollback_lines: 10_000,
            cursor: CursorConfig::default(),
            opacity: 1.0,
            padding: PaddingConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn load_or_default(path: &std::path::Path) -> Self {
        Self::load(path).unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Default config file path: %APPDATA%/TermForge/termforge.toml
    pub fn default_path() -> std::path::PathBuf {
        let appdata = std::env::var("APPDATA")
            .unwrap_or_else(|_| std::env::var("USERPROFILE").unwrap_or_default());
        std::path::PathBuf::from(appdata)
            .join("TermForge")
            .join("termforge.toml")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Cascadia Code".to_string(),
            size: 13.0,
            line_height: 1.4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorScheme {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection: String,
    /// Named color theme: "dark" (default), or a custom name.
    pub theme: String,
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self {
            background: "#0d1117".to_string(),
            foreground: "#e6edf3".to_string(),
            cursor: "#e6edf3".to_string(),
            selection: "#264f78".to_string(),
            theme: "dark".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Shell executable path. Empty = auto-detect (pwsh > powershell > cmd).
    pub path: String,
    pub args: Vec<String>,
    /// Automatically source TermForge shell integration on startup.
    pub auto_integration: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            args: Vec::new(),
            auto_integration: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
    /// Show sidebar on startup.
    pub sidebar_visible: bool,
    /// Default workspace name.
    pub default_workspace: String,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 768,
            sidebar_visible: true,
            default_workspace: "work / backend".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    /// "block", "bar", "underline"
    pub style: String,
    pub blink: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: "block".to_string(),
            blink: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PaddingConfig {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

impl Default for PaddingConfig {
    fn default() -> Self {
        Self {
            top: 8,
            bottom: 8,
            left: 12,
            right: 12,
        }
    }
}
