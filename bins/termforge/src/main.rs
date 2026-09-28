//! TermForge desktop app.
//!
//! Phase 1: one terminal per window, rendered with GPUI. The session runs
//! in-process; Phase 2 moves it behind `forged` (docs/adr/0005).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod fonts;
mod terminal_view;

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
    let profile = tf_pty::discover_shells()
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no shell found"))?;
    let cwd = std::env::current_dir()
        .ok()
        .filter(|d| d.parent().is_some())
        .or_else(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()));
    Ok(SpawnOptions {
        profile,
        cwd,
        size: GridSize::new(120, 30),
        env: Vec::new(),
        integration_dir: data_dir().map(|d| d.join("shell")),
    })
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

    Application::new().run(move |cx: &mut App| {
        let theme = Arc::new(Theme::generate(ThemeInput::DARK));
        let font = MonoFont::detect(cx);
        let bounds = Bounds::centered(None, size(px(1100.0), px(720.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("TermForge".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(360.0), px(220.0))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| TerminalView::new(spawn, theme, font, window, cx)),
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
