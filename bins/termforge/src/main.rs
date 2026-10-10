//! TermForge desktop app.
//!
//! GPUI desktop client for project-scoped sessions hosted by `forged`.
//! An embedded session remains available only as an explicit development
//! fallback (`TERMFORGE_EMBED_SESSION=1`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod fonts;
mod paint;
mod panes;
mod remote_session;
mod suggest;
mod terminal_view;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};
use tf_engine::GridSize;
use tf_session::SpawnOptions;
use tf_ui::{Theme, ThemeInput};

use crate::fonts::MonoFont;
use crate::terminal_view::TerminalView;

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("TERMFORGE_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            directories::ProjectDirs::from("dev", "TermForge", "TermForge")
                .map(|d| d.data_local_dir().to_path_buf())
        })
}

fn spawn_options() -> anyhow::Result<SpawnOptions> {
    let ssh_host = std::env::var("TERMFORGE_SSH_HOST").ok();
    let profile = match ssh_host.as_deref() {
        Some(host) => tf_pty::ssh_profile(host)
            .ok_or_else(|| anyhow::anyhow!("invalid SSH host or ssh not found: {host}"))?,
        None => tf_pty::discover_shells()
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no shell found"))?,
    };
    let cwd = if ssh_host.is_some() {
        None
    } else {
        std::env::current_dir()
            .ok()
            .filter(|d| d.parent().is_some())
            .or_else(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
    };
    let mut env = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(bundle_bin) = executable.parent() {
            let mut paths = vec![bundle_bin.to_path_buf()];
            if let Some(existing) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            if let Ok(path) = std::env::join_paths(paths) {
                env.push(("PATH".into(), path.to_string_lossy().into_owned()));
            }
        }
    }
    Ok(SpawnOptions {
        profile,
        ssh_host,
        cwd,
        size: GridSize::new(120, 30),
        env,
        integration_dir: data_dir().map(|d| d.join("shell")),
    })
}

/// Resolve the repository root once so every session created by this window
/// carries a stable project identity. Outside a repository, the launch
/// directory itself is the project root.
fn project_root(cwd: Option<&std::path::Path>) -> Option<PathBuf> {
    let cwd = cwd?;
    let root = cwd
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(cwd);
    dunce::canonicalize(root).ok()
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TERMFORGE_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let spawn = match spawn_options() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("{e:#}");
            std::process::exit(1);
        }
    };
    let project_root = if spawn.ssh_host.is_some() {
        None
    } else {
        project_root(spawn.cwd.as_deref())
    };

    Application::new().run(move |cx: &mut App| {
        let theme = Arc::new(Theme::generate(ThemeInput::DARK));
        let font = MonoFont::detect(cx);
        let bounds = Bounds::centered(None, size(px(1100.0), px(720.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("dev.termforge.TermForge".into()),
                titlebar: Some(TitlebarOptions {
                    title: Some("TermForge".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(360.0), px(220.0))),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| TerminalView::new(spawn, project_root, theme, font, window, cx))
            },
        );
        if let Err(e) = opened {
            tracing::error!("failed to open window: {e:#}");
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
