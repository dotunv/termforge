//! The Timeline area: an activity log of observed changes, with filtering.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::activity::{self, Level, Link, Source};
use crate::operate::{ContainerProbe, ContainerState, PortProbe};

/// What was last seen, so only changes become events. The first observation
/// of anything is a baseline, not an event.
#[derive(Default)]
pub(in crate::terminal_view) struct ActivityBaseline {
    services: Option<HashMap<u64, surface::ProcessState>>,
    ports: Option<HashSet<u16>>,
    containers: Option<HashMap<String, (String, ContainerState)>>,
    daemon_seen: usize,
    diagnostics: Option<HashSet<(std::path::PathBuf, u32, String)>>,
}

impl TerminalView {
    /// Record port openings and closings inside the project.
    pub(in crate::terminal_view) fn note_ports(&mut self, probe: &PortProbe) {
        let PortProbe::Scanned(ports) = probe else {
            return;
        };
        let now: HashSet<u16> = ports
            .iter()
            .filter(|p| p.in_project)
            .map(|p| p.port)
            .collect();
        if let Some(before) = self.activity_baseline.ports.take() {
            for port in now.difference(&before) {
                let owner = ports
                    .iter()
                    .find(|p| p.port == *port)
                    .and_then(|p| p.process.clone())
                    .unwrap_or_else(|| "unknown owner".into());
                self.activity.record(
                    Source::Port,
                    Level::Info,
                    format!("port :{port} opened ({owner})"),
                );
            }
            for port in before.difference(&now) {
                self.activity
                    .record(Source::Port, Level::Info, format!("port :{port} closed"));
            }
        }
        self.activity_baseline.ports = Some(now);
    }

    pub(in crate::terminal_view) fn note_containers(&mut self, probe: &ContainerProbe) {
        let ContainerProbe::Listed { containers, .. } = probe else {
            return;
        };
        let now: HashMap<String, (String, ContainerState)> = containers
            .iter()
            .map(|c| (c.id.clone(), (c.name.clone(), c.state.clone())))
            .collect();
        if let Some(before) = self.activity_baseline.containers.take() {
            for (id, (name, state)) in &now {
                match before.get(id) {
                    None => self.activity.record(
                        Source::Container,
                        Level::Info,
                        format!("{name}: appeared ({})", state.label()),
                    ),
                    Some((_, old)) if old != state => {
                        let level = if matches!(state, ContainerState::Exited) {
                            Level::Warn
                        } else {
                            Level::Info
                        };
                        self.activity.record(
                            Source::Container,
                            level,
                            format!("{name}: {} → {}", old.label(), state.label()),
                        );
                    }
                    Some(_) => {}
                }
            }
            for (id, (name, _)) in &before {
                if !now.contains_key(id) {
                    self.activity.record(
                        Source::Container,
                        Level::Info,
                        format!("{name}: removed"),
                    );
                }
            }
        }
        self.activity_baseline.containers = Some(now);
    }

    /// Turn observed changes into activity events. Runs on the UI tick and is
    /// cheap: it only compares against the previous observation.
    pub(in crate::terminal_view) fn record_activity(&mut self, cx: &mut Context<'_, Self>) {
        let before = self.activity.len();

        // Services TermForge started.
        let now: HashMap<u64, surface::ProcessState> = self
            .tabs
            .iter()
            .filter_map(|tab| Some((tab.id, Self::managed_state(tab)?)))
            .collect();
        if let Some(previous) = self.activity_baseline.services.take() {
            for tab in &self.tabs {
                let Some(state) = now.get(&tab.id) else {
                    continue;
                };
                let name = tab
                    .name
                    .clone()
                    .or_else(|| tab.managed.as_ref().map(|m| m.command.clone()))
                    .unwrap_or_default();
                let (message, level) = match previous.get(&tab.id) {
                    None => (format!("{name}: started ({})", state.label()), Level::Info),
                    Some(old) if old != state => (
                        format!("{name}: {} → {}", old.label(), state.label()),
                        match state {
                            surface::ProcessState::Failed(_) => Level::Error,
                            surface::ProcessState::SessionEnded(_) => Level::Warn,
                            _ => Level::Info,
                        },
                    ),
                    Some(_) => continue,
                };
                self.activity
                    .record_linked(Source::Service, level, message, Link::Tab(tab.id));
            }
        }
        self.activity_baseline.services = Some(now);

        // Daemon connection changes.
        let history = remote_session::daemon_history();
        for (at, line) in history.iter().skip(self.activity_baseline.daemon_seen) {
            self.activity.push(activity::Event {
                at: *at,
                source: Source::Daemon,
                level: if line.contains("offline") || line.contains("!=") {
                    Level::Warn
                } else {
                    Level::Info
                },
                message: line.clone(),
                link: None,
            });
        }
        self.activity_baseline.daemon_seen = history.len();

        // Language-server diagnostics that newly appeared.
        let current: HashSet<(std::path::PathBuf, u32, String)> = self
            .lsp_diagnostics
            .iter()
            .map(|d| (d.path.clone(), d.start.line, d.message.clone()))
            .collect();
        if let Some(seen) = self.activity_baseline.diagnostics.take() {
            for diagnostic in &self.lsp_diagnostics {
                let key = (
                    diagnostic.path.clone(),
                    diagnostic.start.line,
                    diagnostic.message.clone(),
                );
                if seen.contains(&key) {
                    continue;
                }
                let level = match diagnostic.severity {
                    crate::lsp::Severity::Error => Level::Error,
                    crate::lsp::Severity::Warning => Level::Warn,
                    _ => Level::Info,
                };
                let file = diagnostic
                    .path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                self.activity.record_linked(
                    Source::Language,
                    level,
                    format!(
                        "{file}:{}: {}",
                        diagnostic.start.line + 1,
                        diagnostic.message
                    ),
                    Link::File(diagnostic.path.clone(), diagnostic.start.line),
                );
            }
        }
        self.activity_baseline.diagnostics = Some(current);

        if self.activity.len() != before {
            cx.notify();
        }
    }

    /// Go to what an activity event is about.
    pub(super) fn open_activity_link(&mut self, link: Link, cx: &mut Context<'_, Self>) {
        match link {
            Link::Tab(id) => {
                if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                    self.select_tab(index, cx);
                    self.switch_workspace(Workspace::Build, cx);
                }
            }
            Link::File(path, line) => {
                let found = self
                    .lsp_diagnostics
                    .iter()
                    .find(|d| d.path == path && d.start.line == line)
                    .cloned();
                if let Some(diagnostic) = found {
                    self.open_diagnostic(&diagnostic, cx);
                }
            }
        }
    }

    /// Keys for the Timeline filter box while it has focus.
    pub(in crate::terminal_view) fn activity_filter_key(
        &mut self,
        ev: &KeyDownEvent,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        match ks.key.as_str() {
            "escape" | "enter" => self.activity_filter_focus = false,
            "backspace" => {
                if m.control {
                    self.activity_filter.text.clear();
                } else {
                    self.activity_filter.text.pop();
                }
            }
            "v" if m.control => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.activity_filter
                        .text
                        .push_str(text.lines().next().unwrap_or_default());
                }
            }
            _ if !m.control && !m.alt => match ks.key_char.as_deref() {
                Some(ch) if !ch.chars().any(char::is_control) => {
                    self.activity_filter.text.push_str(ch);
                }
                _ => return false,
            },
            _ => return false,
        }
        cx.notify();
        true
    }

    /// The activity log with a text filter, source toggles and a level
    /// threshold. Events that point at something are clickable.
    pub(in crate::terminal_view) fn render_area_timeline(
        &self,
        id: AreaId,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let filter = &self.activity_filter;
        let visible = self.activity.visible(filter);
        let shown = visible.len();
        let total = self.activity.len();
        let focused = self.activity_filter_focus;
        let chip = |label: &'static str, on: bool, color: Rgb| {
            div()
                .px(px(6.0))
                .py(px(1.0))
                .rounded(px(tokens::radius::SM))
                .border_1()
                .border_color(hsla(if on { color } else { t.border }))
                .text_size(px(tokens::text::XS))
                .text_color(hsla(if on { color } else { t.text_faint }))
                .cursor_pointer()
                .child(label)
        };
        let empty = if total == 0 {
            "No activity yet. Events appear when a service, port, container, language \
             server or the daemon connection changes."
        } else {
            "No events match the filter."
        };
        let filter_text = if filter.text.is_empty() && !focused {
            SharedString::from("filter activity…")
        } else {
            SharedString::from(format!("{}{}", filter.text, if focused { "▏" } else { "" }))
        };

        div()
            .id(("area-timeline", id))
            .size_full()
            .p(px(tokens::space::SM))
            .bg(hsla(t.bg_panel))
            .flex()
            .flex_col()
            .gap(px(tokens::space::XS))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(tokens::space::XS))
                    .child(
                        div()
                            .id(("activity-filter", id))
                            .min_w(px(150.0))
                            .px(px(tokens::space::SM))
                            .py(px(2.0))
                            .rounded(px(tokens::radius::SM))
                            .border_1()
                            .border_color(hsla(if focused { t.accent } else { t.border }))
                            .bg(hsla(t.bg_app))
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(if filter.text.is_empty() {
                                t.text_faint
                            } else {
                                t.text
                            }))
                            .cursor_text()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.activity_filter_focus = true;
                                cx.notify();
                            }))
                            .child(filter_text),
                    )
                    .children(Source::ALL.into_iter().map(|source| {
                        chip(source.label(), filter.shows(source), t.accent)
                            .id(("activity-source", source as usize))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.activity_filter.toggle(source);
                                cx.notify();
                            }))
                    }))
                    .child(
                        chip(
                            filter.min_level.label(),
                            filter.min_level != Level::Info,
                            t.warning,
                        )
                        .id(("activity-level", id))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.activity_filter.min_level = this.activity_filter.min_level.cycle();
                            cx.notify();
                        })),
                    )
                    .when(filter.is_active(), |d| {
                        d.child(
                            chip("clear", true, t.text_muted)
                                .id(("activity-clear", id))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.activity_filter.clear();
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(SharedString::from(format!("{shown} of {total}"))),
                    ),
            )
            .child(
                div()
                    .id(("activity-list", id))
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .when(visible.is_empty(), |d| d.child(self.muted_line(empty)))
                    .children(visible.into_iter().enumerate().map(|(row, event)| {
                        let color = match event.level {
                            Level::Info => t.accent,
                            Level::Warn => t.warning,
                            Level::Error => t.danger,
                        };
                        let link = event.link.clone();
                        div()
                            .id(("activity-event", row))
                            .px(px(tokens::space::SM))
                            .py(px(4.0))
                            .rounded(px(tokens::radius::SM))
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .text_size(px(tokens::text::XS))
                            .when_some(link, |d, link| {
                                d.cursor_pointer()
                                    .hover(|style| style.bg(hsla(t.bg_hover)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_activity_link(link.clone(), cx);
                                    }))
                            })
                            .child(
                                div()
                                    .w(px(46.0))
                                    .flex_none()
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(ago(event.at))),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px(px(5.0))
                                    .rounded(px(tokens::radius::SM))
                                    .bg(with_alpha(hsla(color), 0.14))
                                    .text_color(hsla(color))
                                    .child(event.source.label()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(hsla(t.text_muted))
                                    .child(SharedString::from(event.message.clone())),
                            )
                    })),
            )
    }
}
