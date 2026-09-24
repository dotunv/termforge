//! TermForge desktop app.
//!
//! Phase 0: a themed window that proves the GPUI toolchain, the design
//! tokens and the Windows build. The terminal view arrives in Phase 1.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, Rgba, Window, WindowBounds,
    WindowOptions,
};
use tf_ui::{tokens, Rgb, Theme, ThemeInput};

fn color(c: Rgb) -> Rgba {
    rgb(u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b))
}

struct Root {
    theme: Theme,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(color(t.bg_app))
            .text_color(color(t.text))
            .text_size(px(tokens::text::MD))
            .child(
                div()
                    .h(px(tokens::layout::TITLEBAR_H))
                    .px(px(tokens::space::LG))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(color(t.border))
                    .child("TermForge"),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color(t.text_muted))
                    .child("Phase 0 foundation. Terminal view lands in Phase 1."),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1100.0), px(720.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Root {
                    theme: Theme::generate(ThemeInput::DARK),
                })
            },
        );
        if let Err(e) = opened {
            eprintln!("failed to open window: {e:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
