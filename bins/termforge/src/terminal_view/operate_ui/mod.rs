//! Operate workspace areas: where work runs, what is running, and what is
//! listening. Everything shown is observed; where the answer cannot be known
//! (a remote host, a process owned by another user, a lost daemon) the area
//! says so.
//!
//! One file per area: `environment`, `services` (with `containers`) and
//! `timeline`. Shared helpers live here.

use std::time::{Duration, SystemTime};

use super::*;
use crate::operate::{self, PortProbe};

mod containers;
mod environment;
mod services;
mod timeline;

pub(in crate::terminal_view) use timeline::ActivityBaseline;

/// Ports shown from outside the project before the rest are summarised.
const OTHER_PORTS_SHOWN: usize = 8;
const PORT_REFRESH: Duration = Duration::from_secs(3);
const CONTAINER_REFRESH: Duration = Duration::from_secs(6);

fn ago(at: SystemTime) -> String {
    match at.elapsed() {
        Ok(elapsed) if elapsed.as_secs() < 60 => format!("{}s ago", elapsed.as_secs()),
        Ok(elapsed) if elapsed.as_secs() < 3600 => format!("{}m ago", elapsed.as_secs() / 60),
        Ok(elapsed) => format!("{}h ago", elapsed.as_secs() / 3600),
        Err(_) => "just now".into(),
    }
}

impl TerminalView {
    /// Whether the visible layout has an area that shows listening ports.
    pub(in crate::terminal_view) fn shows_services(&self) -> bool {
        let tree = if self.workspace.hosts_sessions() {
            self.tabs.get(self.active_tab).map(|tab| &tab.layout)
        } else {
            self.workspace_layouts.get(&self.workspace)
        };
        tree.is_some_and(|tree| {
            tree.areas()
                .iter()
                .any(|area| area.editor.tag() == EditorKindTag::Services)
        })
    }

    /// Rescan listening ports when a Services area is visible and the last
    /// scan is stale. The scan runs off the UI thread.
    pub(in crate::terminal_view) fn refresh_ports_if_stale(&mut self, cx: &mut Context<'_, Self>) {
        if !self.shows_services() || self.ports_scanning {
            return;
        }
        if self
            .ports_at
            .is_some_and(|at| at.elapsed().unwrap_or_default() < PORT_REFRESH)
        {
            return;
        }
        self.refresh_ports(cx);
    }

    pub(in crate::terminal_view) fn refresh_ports(&mut self, cx: &mut Context<'_, Self>) {
        if self.spawn.ssh_host.is_some() {
            self.ports = Some(PortProbe::Unsupported(
                "ports are read from this machine, not the remote host".into(),
            ));
            self.ports_at = Some(SystemTime::now());
            return;
        }
        self.ports_scanning = true;
        let root = self
            .local_project_root
            .clone()
            .or_else(|| self.project_root.clone());
        cx.spawn(async move |this, cx| {
            let probe = cx
                .background_executor()
                .spawn(async move { operate::scan_ports(root.as_deref()) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.note_ports(&probe);
                view.ports = Some(probe);
                view.ports_at = Some(SystemTime::now());
                view.ports_scanning = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn operate_section(&self, title: &'static str) -> gpui::Div {
        div()
            .pt(px(tokens::space::SM))
            .pb(px(2.0))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(self.theme.text_faint))
            .child(title)
    }

    pub(super) fn operate_row(&self, label: &'static str, value: String) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .gap(px(tokens::space::SM))
            .text_size(px(tokens::text::SM))
            .child(
                div()
                    .w(px(78.0))
                    .flex_none()
                    .text_color(hsla(t.text_faint))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_color(hsla(t.text))
                    .child(SharedString::from(value)),
            )
    }

    pub(super) fn muted_line(&self, text: &str) -> gpui::AnyElement {
        div()
            .px(px(tokens::space::SM))
            .py(px(2.0))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(self.theme.text_faint))
            .child(SharedString::from(text.to_owned()))
            .into_any_element()
    }

    pub(super) fn operate_section_dyn(&self, title: String) -> gpui::Div {
        div()
            .pt(px(tokens::space::SM))
            .pb(px(2.0))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(self.theme.text_faint))
            .child(SharedString::from(title))
    }
}
