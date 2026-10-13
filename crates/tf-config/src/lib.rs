//! TermForge configuration system.
//!
//! - TOML file with accompanying JSON schema for editor autocompletion
//! - Hot reload on file change
//! - Validation with error surfacing (not panicking)
//! - Defaults for all settings

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "tf-ui")]
pub mod theme;

use directories::ProjectDirs;
use notify::{
    Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{info, warn};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config directory not found")]
    NoConfigDir,
    #[error("failed to read config file: {0}")]
    Read(std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(toml::de::Error),
    #[error("failed to write config: {0}")]
    Write(std::io::Error),
    #[error("failed to serialize config: {0}")]
    Serialize(toml::ser::Error),
    #[error("config validation failed: {0}")]
    Validation(String),
    #[error("watcher error: {0}")]
    Watcher(#[from] notify::Error),
}

pub type Result<T> = std::result::Result<T, ConfigError>;

/// Global configuration for TermForge.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Terminal appearance settings
    pub terminal: TerminalConfig,
    /// Key bindings
    pub keys: KeyConfig,
    /// Shell integration settings
    pub shell: ShellConfig,
    /// UI settings
    pub ui: UiConfig,
    /// Performance settings
    pub performance: PerformanceConfig,
    /// Client/account identities that can be assigned to projects
    #[serde(default = "default_contexts")]
    pub contexts: Vec<ContextConfig>,
}

fn default_contexts() -> Vec<ContextConfig> {
    vec![ContextConfig::default()]
}

impl Default for Config {
    fn default() -> Self {
        Self {
            terminal: TerminalConfig::default(),
            keys: KeyConfig::default(),
            shell: ShellConfig::default(),
            ui: UiConfig::default(),
            performance: PerformanceConfig::default(),
            contexts: default_contexts(),
        }
    }
}

impl Config {
    /// Load configuration from the default location, creating it if it doesn't exist.
    pub async fn load_or_default() -> Result<ConfigHandle> {
        let path = config_path()?;
        Self::load_or_create(path).await
    }

    /// Load configuration from a specific path, creating it if it doesn't exist.
    pub async fn load_or_create(path: PathBuf) -> Result<ConfigHandle> {
        let config = if path.exists() {
            Self::load_from_path(&path).await?
        } else {
            let default = Self::default();
            default.save_to_path(&path).await?;
            default
        };
        ConfigHandle::new(config, path).await
    }

    /// Load configuration from a specific path.
    pub async fn load_from_path(path: &Path) -> Result<Self> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConfigError::Read)?;
        Self::parse(&content)
    }

    /// Save configuration to a specific path.
    pub async fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(ConfigError::Write)?;
        }
        let content = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        tokio::fs::write(path, content)
            .await
            .map_err(ConfigError::Write)?;
        Ok(())
    }

    /// Parse and validate configuration text. Never panics, whatever the input.
    pub fn parse(text: &str) -> Result<Config> {
        let config: Config = toml::from_str(text).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    /// Load synchronously, for callers that have no async runtime (the GUI).
    pub fn load_sync(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(ConfigError::Read)?;
        Self::parse(&content)
    }

    /// Save synchronously. Writes a sibling temp file and renames it so a
    /// watcher or a crash never observes a half-written config.
    pub fn save_sync(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::Write)?;
        }
        let content = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, content).map_err(ConfigError::Write)?;
        std::fs::rename(&tmp, path).map_err(ConfigError::Write)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<()> {
        if self.terminal.font_size < 6.0 || self.terminal.font_size > 72.0 {
            return Err(ConfigError::Validation(
                "font_size must be between 6 and 72".into(),
            ));
        }
        if self.terminal.scrollback_limit == 0 {
            return Err(ConfigError::Validation(
                "scrollback_limit must be > 0".into(),
            ));
        }
        if !(1.0..=2.5).contains(&self.terminal.line_height) {
            return Err(ConfigError::Validation(
                "line_height must be between 1.0 and 2.5".into(),
            ));
        }
        if !self.ui.accent.is_empty() && tf_ui_hex(&self.ui.accent).is_none() {
            return Err(ConfigError::Validation(
                "ui.accent must be empty or #rrggbb".into(),
            ));
        }
        if self.ui.contrast.is_some_and(|c| c > 100) {
            return Err(ConfigError::Validation(
                "ui.contrast must be between 0 and 100".into(),
            ));
        }
        if !(0.5..=1.0).contains(&self.ui.opacity) {
            return Err(ConfigError::Validation(
                "ui.opacity must be between 0.5 and 1.0".into(),
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for context in &self.contexts {
            if !valid_context_id(&context.id) {
                return Err(ConfigError::Validation(format!(
                    "context id `{}` must contain only lowercase letters, digits and hyphens",
                    context.id
                )));
            }
            if !ids.insert(context.id.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate context id `{}`",
                    context.id
                )));
            }
            if context.name.trim().is_empty() || context.name.len() > 80 {
                return Err(ConfigError::Validation(format!(
                    "context `{}` must have a name of at most 80 characters",
                    context.id
                )));
            }
            if !context.color.is_empty() && tf_ui_hex(&context.color).is_none() {
                return Err(ConfigError::Validation(format!(
                    "context `{}` color must be empty or #rrggbb",
                    context.id
                )));
            }
            for (key, value) in context.shell_env() {
                if key.as_bytes().contains(&0) || value.as_bytes().contains(&0) {
                    return Err(ConfigError::Validation(format!(
                        "context `{}` environment contains NUL",
                        context.id
                    )));
                }
            }
        }
        Ok(())
    }

    /// Get the default config directory. `TERMFORGE_CONFIG_DIR` overrides it,
    /// which keeps tests and portable installs off the user's real config.
    pub fn config_dir() -> Result<PathBuf> {
        if let Some(dir) = std::env::var_os("TERMFORGE_CONFIG_DIR") {
            return Ok(PathBuf::from(dir));
        }
        ProjectDirs::from("dev", "TermForge", "TermForge")
            .map(|d| d.config_local_dir().to_path_buf())
            .ok_or(ConfigError::NoConfigDir)
    }

    /// Get the default config file path.
    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    /// Directory scanned for `*.toml` theme files.
    pub fn themes_dir() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("themes"))
    }

    /// Directory scanned for `.ttf`, `.otf` and `.ttc` font files.
    pub fn fonts_dir() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("fonts"))
    }

    /// Get the JSON schema path.
    pub fn schema_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.schema.json"))
    }

    /// Write the JSON schema to the default location.
    pub fn write_schema() -> Result<()> {
        let path = Self::schema_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::Write)?;
        }
        let schema = schemars::schema_for!(Config);
        let json = serde_json::to_string_pretty(&schema)
            .map_err(|e| ConfigError::Validation(e.to_string()))?;
        std::fs::write(path, json).map_err(ConfigError::Write)?;
        #[cfg(feature = "tf-ui")]
        {
            let theme = schemars::schema_for!(theme::ThemeSpec);
            let json = serde_json::to_string_pretty(&theme)
                .map_err(|e| ConfigError::Validation(e.to_string()))?;
            std::fs::write(Self::config_dir()?.join("theme.schema.json"), json)
                .map_err(ConfigError::Write)?;
        }
        Ok(())
    }
}

/// `#rrggbb` check that does not need the optional `tf-ui` dependency.
fn tf_ui_hex(s: &str) -> Option<()> {
    let digits = s.strip_prefix('#').unwrap_or(s);
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit())).then_some(())
}

fn valid_context_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A client/account identity. Secrets and credentials remain in Git, SSH and
/// GitHub CLI; TermForge stores only paths and expected public identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ContextConfig {
    /// Stable identifier persisted with projects.
    pub id: String,
    /// Human-readable name shown prominently in the UI.
    pub name: String,
    /// Badge colour as `#rrggbb`.
    pub color: String,
    /// Expected and injected Git author/committer name.
    pub git_name: String,
    /// Expected and injected Git author/committer email.
    pub git_email: String,
    /// Expected host in the repository's origin URL, such as github.com.
    pub github_host: String,
    /// Optional GitHub CLI config directory for this account.
    pub gh_config_dir: Option<PathBuf>,
    /// Optional dedicated global Git config for this identity.
    pub git_config_global: Option<PathBuf>,
}

impl ContextConfig {
    /// Environment applied to newly-created shells. Authentication remains in
    /// external credential managers; these values only select an identity.
    pub fn shell_env(&self) -> Vec<(String, String)> {
        let mut env = vec![("TERMFORGE_CONTEXT".into(), self.id.clone())];
        if !self.git_name.is_empty() {
            env.push(("GIT_AUTHOR_NAME".into(), self.git_name.clone()));
            env.push(("GIT_COMMITTER_NAME".into(), self.git_name.clone()));
        }
        if !self.git_email.is_empty() {
            env.push(("GIT_AUTHOR_EMAIL".into(), self.git_email.clone()));
            env.push(("GIT_COMMITTER_EMAIL".into(), self.git_email.clone()));
        }
        if let Some(path) = &self.gh_config_dir {
            env.push(("GH_CONFIG_DIR".into(), path.to_string_lossy().into_owned()));
        }
        if let Some(path) = &self.git_config_global {
            env.push((
                "GIT_CONFIG_GLOBAL".into(),
                path.to_string_lossy().into_owned(),
            ));
        }
        env
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            id: "personal".into(),
            name: "Personal".into(),
            color: "#8b5cf6".into(),
            git_name: String::new(),
            git_email: String::new(),
            github_host: "github.com".into(),
            gh_config_dir: None,
            git_config_global: None,
        }
    }
}

fn config_path() -> Result<PathBuf> {
    Config::config_path()
}

/// A handle to the configuration that supports hot reload.
#[derive(Debug)]
pub struct ConfigHandle {
    config: Arc<RwLock<Config>>,
    path: PathBuf,
    _watcher: RecommendedWatcher,
    _reload_tx: mpsc::UnboundedSender<()>,
    _reload_task: tokio::task::JoinHandle<()>,
    changes: broadcast::Sender<Config>,
}

impl ConfigHandle {
    async fn new(config: Config, path: PathBuf) -> Result<Self> {
        let config = Arc::new(RwLock::new(config));
        let config_clone = config.clone();
        let path_clone = path.clone();
        let (changes, _) = broadcast::channel(16);
        let reload_changes = changes.clone();

        let (reload_tx, mut reload_rx) = mpsc::unbounded_channel();

        // Background task that handles config reloads
        let reload_task = tokio::spawn(async move {
            while reload_rx.recv().await.is_some() {
                match Self::reload_config(config_clone.clone(), &path_clone).await {
                    Ok(config) => {
                        let _ = reload_changes.send(config);
                    }
                    Err(e) => warn!("failed to reload config: {e}"),
                }
            }
        });

        let path_for_watcher = path.clone();
        let reload_tx_for_watcher = reload_tx.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
                        && event.paths.iter().any(|p| p == &path_for_watcher)
                    {
                        let _ = reload_tx_for_watcher.send(());
                    }
                }
            },
            NotifyConfig::default(),
        )?;

        watcher.watch(&path, RecursiveMode::NonRecursive)?;

        // Also watch the parent directory for file creation (some editors write to temp then rename)
        if let Some(parent) = path.parent() {
            let _ = watcher.watch(parent, RecursiveMode::NonRecursive);
        }

        Ok(Self {
            config,
            path,
            _watcher: watcher,
            _reload_tx: reload_tx,
            _reload_task: reload_task,
            changes,
        })
    }

    async fn reload_config(config: Arc<RwLock<Config>>, path: &Path) -> Result<Config> {
        // Debounce: wait a bit for the write to complete
        tokio::time::sleep(Duration::from_millis(50)).await;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(ConfigError::Read)?;
        let new_config = Config::parse(&content)?;

        let mut guard = config.write().await;
        *guard = new_config.clone();
        info!("config reloaded from {}", path.display());
        Ok(new_config)
    }

    /// Get a snapshot of the current configuration.
    pub async fn get(&self) -> Config {
        self.config.read().await.clone()
    }

    /// Update the configuration and persist it.
    pub async fn set(&self, f: impl FnOnce(&mut Config)) -> Result<()> {
        let mut guard = self.config.write().await;
        f(&mut guard);
        guard.validate()?;
        guard.save_to_path(&self.path).await?;
        let _ = self.changes.send(guard.clone());
        Ok(())
    }

    /// Get a clone of the current configuration (synchronous, for use in non-async contexts).
    /// Note: This blocks on the lock.
    pub fn get_blocking(&self) -> Config {
        self.config.blocking_read().clone()
    }

    /// Subscribe to configuration changes.
    pub fn subscribe(&self) -> broadcast::Receiver<Config> {
        self.changes.subscribe()
    }
}

/// Terminal appearance and behavior settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    /// Font family (empty = auto-detect monospace)
    pub font_family: String,
    /// Font size in points
    pub font_size: f32,
    /// Line height multiplier
    pub line_height: f32,
    /// Scrollback buffer limit (lines)
    pub scrollback_limit: usize,
    /// Enable ligatures
    pub ligatures: bool,
    /// Tint default-colour bold text with the active theme accent
    pub accent_bold: bool,
    /// Cursor style
    pub cursor_style: CursorStyleConfig,
    /// Cursor blink
    pub cursor_blink: bool,
    /// Bell behavior
    pub bell: BellConfig,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_size: 14.0,
            line_height: 1.35,
            scrollback_limit: 10000,
            // The terminal grid has always drawn without ligatures; keep
            // that unless the user opts in.
            ligatures: false,
            accent_bold: true,
            cursor_style: CursorStyleConfig::Block,
            cursor_blink: true,
            bell: BellConfig::default(),
        }
    }
}

/// Cursor style options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyleConfig {
    Block,
    Underline,
    Bar,
}

/// Bell behavior options.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct BellConfig {
    /// Enable visual bell
    pub visual: bool,
    /// Enable audio bell
    pub audio: bool,
}

impl Default for BellConfig {
    fn default() -> Self {
        Self {
            visual: true,
            audio: false,
        }
    }
}

/// Key binding configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct KeyConfig {
    /// Leader key (e.g., "Ctrl+Shift")
    pub leader: String,
    /// Custom key bindings (action -> key)
    pub bindings: std::collections::HashMap<String, String>,
}

impl Default for KeyConfig {
    fn default() -> Self {
        let mut bindings = std::collections::HashMap::new();
        bindings.insert("new_tab".into(), "Ctrl+Shift+T".into());
        bindings.insert("close_tab".into(), "Ctrl+Shift+W".into());
        bindings.insert("next_tab".into(), "Ctrl+Tab".into());
        bindings.insert("prev_tab".into(), "Ctrl+Shift+Tab".into());
        bindings.insert("split_horizontal".into(), "Ctrl+Shift+D".into());
        bindings.insert("split_vertical".into(), "Ctrl+Shift+V".into());
        bindings.insert("copy".into(), "Ctrl+Shift+C".into());
        bindings.insert("paste".into(), "Ctrl+Shift+V".into());
        bindings.insert("find".into(), "Ctrl+Shift+F".into());
        bindings.insert("font_increase".into(), "Ctrl+=".into());
        bindings.insert("font_decrease".into(), "Ctrl+-".into());
        bindings.insert("font_reset".into(), "Ctrl+0".into());
        Self {
            leader: "Ctrl+Shift".into(),
            bindings,
        }
    }
}

/// Shell integration settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ShellConfig {
    /// Auto-inject shell integration for supported shells
    pub auto_inject: bool,
    /// Custom shell integration scripts directory
    pub integration_dir: Option<PathBuf>,
    /// Default shell profile (empty = auto-detect)
    pub default_shell: String,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            auto_inject: true,
            integration_dir: None,
            default_shell: String::new(),
        }
    }
}

/// UI settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// Theme mode, used when `theme_name` is empty or not found
    pub theme: ThemeMode,
    /// Name of a built-in or user theme (empty = follow `theme`)
    pub theme_name: String,
    /// Accent colour override as `#rrggbb` (empty = the theme's own)
    pub accent: String,
    /// Contrast override 0-100 (unset = the theme's own)
    pub contrast: Option<u8>,
    /// Interface font family (empty = auto-detect a proportional face)
    pub font_family: String,
    /// Show tab bar
    pub show_tab_bar: bool,
    /// Show status bar
    pub show_status_bar: bool,
    /// Window opacity (0.0-1.0)
    pub opacity: f32,
    /// Titlebar style
    pub titlebar: TitlebarStyle,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            theme_name: String::new(),
            accent: String::new(),
            contrast: None,
            font_family: String::new(),
            show_tab_bar: true,
            show_status_bar: true,
            opacity: 1.0,
            titlebar: TitlebarStyle::Native,
        }
    }
}

/// Theme mode options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

/// Titlebar style options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TitlebarStyle {
    Native,
    Custom,
    Hidden,
}

/// Performance settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PerformanceConfig {
    /// Maximum frame rate (0 = unlimited)
    pub max_fps: u32,
    /// Enable GPU rendering
    pub gpu_rendering: bool,
    /// Texture atlas size
    pub atlas_size: u32,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            max_fps: 0,
            gpu_rendering: true,
            atlas_size: 4096,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn default_config_is_valid() {
        let config = Config::default();
        config.validate().unwrap();
    }

    #[test]
    fn contexts_parse_and_build_identity_environment() {
        let config = Config::parse(
            r##"
                [[contexts]]
                id = "client-a"
                name = "Client A"
                color = "#168aad"
                git_name = "Ada Dev"
                git_email = "ada@client-a.example"
                github_host = "github.com"
                gh_config_dir = "/accounts/client-a/gh"
            "##,
        )
        .unwrap();
        let context = &config.contexts[0];
        assert_eq!(context.id, "client-a");
        assert!(context
            .shell_env()
            .contains(&("GIT_AUTHOR_EMAIL".into(), "ada@client-a.example".into())));
        assert!(context
            .shell_env()
            .contains(&("GH_CONFIG_DIR".into(), "/accounts/client-a/gh".into())));
    }

    #[test]
    fn contexts_reject_unsafe_or_ambiguous_identity() {
        assert!(Config::parse("[[contexts]]\nid = \"Client A\"\nname = \"A\"").is_err());
        assert!(Config::parse(
            "[[contexts]]\nid = \"same\"\nname = \"A\"\n[[contexts]]\nid = \"same\"\nname = \"B\""
        )
        .is_err());
        assert!(
            Config::parse("[[contexts]]\nid = \"client\"\nname = \"A\"\ncolor = \"red\"").is_err()
        );
    }

    #[tokio::test]
    async fn config_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::default();
        config.save_to_path(&path).await.unwrap();

        let loaded = Config::load_from_path(&path).await.unwrap();
        assert_eq!(config.terminal.font_size, loaded.terminal.font_size);
        assert_eq!(
            config.terminal.scrollback_limit,
            loaded.terminal.scrollback_limit
        );
    }

    #[tokio::test]
    async fn config_hot_reload() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::default();
        config.save_to_path(&path).await.unwrap();

        let handle = ConfigHandle::new(config, path.clone()).await.unwrap();
        let original_size = handle.get().await.terminal.font_size;
        let mut changes = handle.subscribe();

        // Modify the config file
        let mut new_config = Config::default();
        new_config.terminal.font_size = 18.0;
        new_config.save_to_path(&path).await.unwrap();

        let reloaded = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let config = changes.recv().await.unwrap();
                if config.terminal.font_size == 18.0 {
                    break config;
                }
            }
        })
        .await
        .expect("config reload timed out");

        assert_eq!(reloaded.terminal.font_size, 18.0);
        assert_ne!(reloaded.terminal.font_size, original_size);
    }

    #[test]
    fn appearance_fields_validate() {
        let ok = Config::parse(
            "[terminal]\nline_height = 1.5\n[ui]\naccent = \"#ff8800\"\ncontrast = 60\ntheme_name = \"Midnight\"\n",
        )
        .unwrap();
        assert_eq!(ok.ui.theme_name, "Midnight");
        assert!(Config::parse("[ui]\naccent = \"orange\"").is_err());
        assert!(Config::parse("[ui]\ncontrast = 101").is_err());
        assert!(Config::parse("[terminal]\nline_height = 0.5").is_err());
    }

    #[test]
    fn sync_save_round_trips_without_leaving_temp_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let mut config = Config::default();
        config.ui.theme_name = "Forest".into();
        config.save_sync(&path).unwrap();
        assert_eq!(Config::load_sync(&path).unwrap().ui.theme_name, "Forest");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn schema_generation() {
        let schema = schemars::schema_for!(Config);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(json.contains("TerminalConfig"));
        assert!(json.contains("KeyConfig"));
        assert!(json.contains("ShellConfig"));
        assert!(json.contains("UiConfig"));
        assert!(json.contains("PerformanceConfig"));
    }
}
