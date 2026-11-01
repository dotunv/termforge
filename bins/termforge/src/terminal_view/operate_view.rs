//! Operate workspace areas: where work runs, what is running, and what is
//! listening. Everything shown is observed; where the answer cannot be known
//! (a remote host, a process owned by another user, a lost daemon) the area
//! says so.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use super::*;
use crate::activity::{self, Level, Link, Source};
use crate::operate::{self, ContainerAction, ContainerProbe, ContainerState, PortProbe, Support};

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
    pub(super) fn shows_services(&self) -> bool {
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
    pub(super) fn refresh_ports_if_stale(&mut self, cx: &mut Context<'_, Self>) {
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

    pub(super) fn refresh_ports(&mut self, cx: &mut Context<'_, Self>) {
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

    fn operate_section(&self, title: &'static str) -> gpui::Div {
        div()
            .pt(px(tokens::space::SM))
            .pb(px(2.0))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(self.theme.text_faint))
            .child(title)
    }

    fn operate_row(&self, label: &'static str, value: String) -> impl IntoElement {
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

    /// Clear identity first, then what is known about connection and what
    /// this environment can and cannot do.
    pub(super) fn render_area_environment(
        &self,
        id: AreaId,
        _cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let remote = self.spawn.ssh_host.clone();
        let (kind, kind_color) = match &remote {
            Some(host) => (format!("SSH · {host}"), t.warning),
            None => ("LOCAL · this machine".to_owned(), t.success),
        };
        let daemon = remote_session::daemon_status();
        let (daemon_line, daemon_color) = match daemon.status_bar_label() {
            Some(label) => (label, t.warning),
            None => (
                format!(
                    "forged connected · protocol v{}",
                    tf_proto::PROTOCOL_VERSION
                ),
                t.success,
            ),
        };
        let project = self
            .local_project_root
            .as_deref()
            .map_or_else(|| "none".to_owned(), |p| p.display().to_string());
        let context = self
            .active_context()
            .map_or_else(|| "none".to_owned(), |c| c.name.clone());
        let branch = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.branch.clone())
            .unwrap_or_else(|| "unknown".into());
        let capabilities = operate::capabilities(remote.as_deref(), self.container_runtime);
        let history = remote_session::daemon_history();

        let mut hosts: Vec<(String, usize)> = Vec::new();
        for tab in &self.tabs {
            let host = tab
                .spawn
                .ssh_host
                .clone()
                .unwrap_or_else(|| "this machine".into());
            match hosts.iter_mut().find(|(name, _)| *name == host) {
                Some((_, count)) => *count += 1,
                None => hosts.push((host, 1)),
            }
        }

        div()
            .id(("area-environment", id))
            .size_full()
            .overflow_y_scroll()
            .p(px(tokens::space::MD))
            .bg(hsla(t.bg_panel))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .px(px(tokens::space::SM))
                    .py(px(4.0))
                    .rounded(px(tokens::radius::SM))
                    .border_1()
                    .border_color(hsla(kind_color))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(kind_color))
                    .child(SharedString::from(kind)),
            )
            .child(self.operate_section("IDENTITY"))
            .child(self.operate_row("Project", self.project_name()))
            .child(self.operate_row("Path", project))
            .child(self.operate_row("Branch", branch))
            .child(self.operate_row("Context", context))
            .child(self.operate_section("CONNECTION"))
            .child(
                div()
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(daemon_color))
                    .child(SharedString::from(daemon_line)),
            )
            .when(daemon.status_bar_label().is_some(), |d| {
                d.child(
                    div()
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(t.text_faint))
                        .child(
                            "Sessions use this window's embedded fallback, so they may not \
                             survive closing it.",
                        ),
                )
            })
            .child(self.operate_section("SESSIONS BY ENVIRONMENT"))
            .children(hosts.into_iter().map(|(host, count)| {
                self.operate_row(
                    "",
                    format!(
                        "{host} · {count} session{}",
                        if count == 1 { "" } else { "s" }
                    ),
                )
            }))
            .child(self.operate_section("CAPABILITIES"))
            .children(capabilities.into_iter().map(|capability| {
                let (mark, color, note) = match capability.support {
                    Support::Available => ("✓", t.success, String::new()),
                    Support::Unavailable(reason) => ("✕", t.text_faint, format!(" — {reason}")),
                };
                div()
                    .flex()
                    .gap(px(tokens::space::SM))
                    .text_size(px(tokens::text::SM))
                    .child(div().w(px(12.0)).text_color(hsla(color)).child(mark))
                    .child(
                        div()
                            .flex_1()
                            .text_color(hsla(if mark == "✓" { t.text } else { t.text_muted }))
                            .child(SharedString::from(format!("{}{note}", capability.name))),
                    )
            }))
            .child(self.operate_section("CONNECTION HISTORY"))
            .children(if history.is_empty() {
                vec![div()
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child("No changes since launch.")]
            } else {
                history
                    .into_iter()
                    .rev()
                    .take(6)
                    .map(|(at, line)| {
                        div()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_muted))
                            .child(SharedString::from(format!("{} · {line}", ago(at))))
                    })
                    .collect()
            })
    }

    /// Sessions and the processes TermForge started, with observed state and
    /// the actions it can really perform, followed by listening ports.
    pub(super) fn render_area_services(
        &self,
        id: AreaId,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let remote = self.spawn.ssh_host.is_some();
        let action = |id: SharedString, label: &'static str, color: Rgb| {
            div()
                .id(id)
                .px(px(tokens::space::XS))
                .rounded(px(tokens::radius::SM))
                .text_size(px(tokens::text::XS))
                .text_color(hsla(color))
                .cursor_pointer()
                .hover(|style| style.bg(with_alpha(hsla(t.text), 0.08)))
                .child(label)
        };

        let rows: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let state = Self::managed_state(tab);
                let ended = tab
                    .session
                    .as_ref()
                    .is_some_and(|s| s.exit_status().is_some());
                (index, tab, state, ended)
            })
            .collect();

        let mut launchers: Vec<(String, String, bool)> = self
            .agent_clis
            .iter()
            .map(|(label, exe)| (format!("Agent: {label}"), (*exe).to_owned(), true))
            .collect();
        launchers.extend(
            self.tasks
                .items
                .iter()
                .filter(|task| task.state != tf_proto::TaskState::Done)
                .filter_map(|task| Some((task.title.clone(), task.command.clone()?, false))),
        );

        let no_launchers = launchers.is_empty();

        let ports_body: Vec<gpui::AnyElement> = match &self.ports {
            None => vec![self.muted_line("Scanning…")],
            Some(PortProbe::Unsupported(reason)) => {
                vec![self.muted_line(&format!("Not available: {reason}."))]
            }
            Some(PortProbe::Scanned(ports)) => {
                let (mine, other): (Vec<_>, Vec<_>) = ports.iter().partition(|p| p.in_project);
                let unattributed = ports.iter().filter(|p| p.pid.is_none()).count();
                let mut out: Vec<gpui::AnyElement> = Vec::new();
                if mine.is_empty() {
                    out.push(self.muted_line("Nothing in this project is listening."));
                }
                let hidden = other.len().saturating_sub(OTHER_PORTS_SHOWN);
                let shown = mine.into_iter().map(|p| (p, true)).chain(
                    other
                        .into_iter()
                        .take(OTHER_PORTS_SHOWN)
                        .map(|p| (p, false)),
                );
                for (n, (port, project)) in shown.enumerate() {
                    let url = format!("http://localhost:{}", port.port);
                    let owner = match (&port.process, port.pid) {
                        (Some(name), Some(pid)) => format!("{name} ({pid})"),
                        _ => "owner unknown".to_owned(),
                    };
                    let place = port
                        .cwd
                        .as_deref()
                        .map(|cwd| format!(" · {}", cwd.display()))
                        .unwrap_or_default();
                    out.push(
                        div()
                            .id(("port-row", n))
                            .h(px(24.0))
                            .px(px(tokens::space::SM))
                            .flex()
                            .items_center()
                            .gap(px(tokens::space::SM))
                            .rounded(px(tokens::radius::SM))
                            .text_size(px(tokens::text::SM))
                            .text_color(hsla(if project { t.text } else { t.text_faint }))
                            .hover(|style| style.bg(with_alpha(hsla(t.text), 0.05)))
                            .child(
                                div()
                                    .w(px(52.0))
                                    .flex_none()
                                    .text_color(hsla(if project { t.accent } else { t.text_faint }))
                                    .child(SharedString::from(format!(":{}", port.port))),
                            )
                            .child(
                                div()
                                    .w(px(120.0))
                                    .flex_none()
                                    .overflow_hidden()
                                    .child(SharedString::from(owner)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(format!(
                                        "{}{place}",
                                        port.reach().label()
                                    ))),
                            )
                            .child(
                                action(format!("port-copy-{n}").into(), "copy url", t.accent)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            url.clone(),
                                        ));
                                    })),
                            )
                            .into_any_element(),
                    );
                }
                if hidden > 0 {
                    out.push(self.muted_line(&format!("+{hidden} more outside this project")));
                }
                if unattributed > 0 {
                    out.push(self.muted_line(&format!(
                        "{unattributed} listener(s) have an owner this user cannot inspect; \
                         project association is by working directory only."
                    )));
                }
                out
            }
        };
        let scan_note = match (self.ports_scanning, self.ports_at) {
            (true, _) => "scanning…".to_owned(),
            (false, Some(at)) => format!("scanned {}", ago(at)),
            (false, None) => String::new(),
        };

        div()
            .id(("area-services", id))
            .size_full()
            .overflow_y_scroll()
            .px(px(tokens::space::SM))
            .py(px(tokens::space::XS))
            .bg(hsla(t.bg_panel))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .child(self.operate_section("SESSIONS, SERVICES AND AGENTS"))
            .children(rows.into_iter().map(|(index, tab, state, ended)| {
                let managed = tab.managed.clone();
                let color = match state {
                    Some(surface::ProcessState::Running) => t.accent,
                    Some(surface::ProcessState::Succeeded) => t.success,
                    Some(surface::ProcessState::Failed(_)) => t.danger,
                    _ => t.text_faint,
                };
                let status = match (&state, ended) {
                    (Some(state), _) => state.label(),
                    (None, true) => "session ended".to_owned(),
                    (None, false) => "shell".to_owned(),
                };
                let can_restart = matches!(
                    state,
                    Some(surface::ProcessState::Succeeded | surface::ProcessState::Failed(_))
                );
                let running = state == Some(surface::ProcessState::Running);
                let tab_id = tab.id;
                let title = managed.as_ref().map_or_else(
                    || Self::tab_label(tab),
                    |m| {
                        format!(
                            "{}{}",
                            if m.agent { "agent · " } else { "" },
                            tab.name.clone().unwrap_or_else(|| m.command.clone())
                        )
                    },
                );
                div()
                    .id(("service-row", tab_id as usize))
                    .h(px(24.0))
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .rounded(px(tokens::radius::SM))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.text_muted))
                    .hover(|style| style.bg(with_alpha(hsla(t.text), 0.05)))
                    .child(div().size(px(6.0)).rounded_full().bg(hsla(color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(SharedString::from(title)),
                    )
                    .child(
                        div()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(color))
                            .child(SharedString::from(status)),
                    )
                    .child(
                        action(
                            format!("service-shell-{tab_id}").into(),
                            "open shell",
                            t.accent,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_tab(index, cx);
                            this.switch_workspace(Workspace::Build, cx);
                        })),
                    )
                    .child(
                        action(
                            format!("service-follow-{tab_id}").into(),
                            "follow",
                            t.accent,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.watch_in_dock(tab_id, cx);
                            this.show_utility_dock = true;
                            cx.notify();
                        })),
                    )
                    .when(running && managed.is_some(), |d| {
                        d.child(
                            action(format!("service-stop-{tab_id}").into(), "stop", t.warning)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.stop_managed(index);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(can_restart, |d| {
                        d.child(
                            action(
                                format!("service-restart-{tab_id}").into(),
                                "restart",
                                t.accent,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.restart_managed(index);
                                    cx.notify();
                                },
                            )),
                        )
                    })
            }))
            .child(self.muted_line(
                "State comes from each shell's command marks. Processes TermForge did not \
                 start are not tracked.",
            ))
            .child(self.operate_section("START"))
            .when(remote, |d| {
                d.child(self.muted_line(
                    "Services start on this machine, not the remote host, so starting is off.",
                ))
            })
            .when(!remote && no_launchers, |d| {
                d.child(self.muted_line(
                    "No tasks with a command, and no agent CLI (claude, codex, gemini) on PATH.",
                ))
            })
            .when(!remote, |d| {
                d.children(
                    launchers
                        .into_iter()
                        .enumerate()
                        .map(|(i, (title, command, agent))| {
                            let label = SharedString::from(format!("▶  {title}"));
                            div()
                                .id(("service-start", i))
                                .h(px(22.0))
                                .px(px(tokens::space::SM))
                                .flex()
                                .items_center()
                                .rounded(px(tokens::radius::SM))
                                .text_size(px(tokens::text::SM))
                                .text_color(hsla(t.text_muted))
                                .cursor_pointer()
                                .hover(|style| style.bg(with_alpha(hsla(t.text), 0.05)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_managed(title.clone(), command.clone(), agent, cx)
                                }))
                                .child(label)
                        }),
                )
            })
            .children(self.containers_section(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.operate_section("LISTENING PORTS (TCP)"))
                    .child(
                        div()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(SharedString::from(scan_note)),
                    ),
            )
            .children(ports_body)
    }

    fn muted_line(&self, text: &str) -> gpui::AnyElement {
        div()
            .px(px(tokens::space::SM))
            .py(px(2.0))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(self.theme.text_faint))
            .child(SharedString::from(text.to_owned()))
            .into_any_element()
    }
}

/// What was last seen, so only changes become events. The first observation
/// of anything is a baseline, not an event.
#[derive(Default)]
pub(super) struct ActivityBaseline {
    services: Option<HashMap<u64, surface::ProcessState>>,
    ports: Option<HashSet<u16>>,
    containers: Option<HashMap<String, (String, ContainerState)>>,
    daemon_seen: usize,
    diagnostics: Option<HashSet<(std::path::PathBuf, u32, String)>>,
}

impl TerminalView {
    /// Record port openings and closings inside the project.
    pub(super) fn note_ports(&mut self, probe: &PortProbe) {
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

    pub(super) fn note_containers(&mut self, probe: &ContainerProbe) {
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
    pub(super) fn record_activity(&mut self, cx: &mut Context<'_, Self>) {
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
    fn open_activity_link(&mut self, link: Link, cx: &mut Context<'_, Self>) {
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
    pub(super) fn activity_filter_key(
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
    pub(super) fn render_area_timeline(
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

    pub(super) fn refresh_containers_if_stale(&mut self, cx: &mut Context<'_, Self>) {
        if !self.shows_services() || self.containers_scanning || self.container_busy.is_some() {
            return;
        }
        if self
            .containers_at
            .is_some_and(|at| at.elapsed().unwrap_or_default() < CONTAINER_REFRESH)
        {
            return;
        }
        self.refresh_containers(cx);
    }

    pub(super) fn refresh_containers(&mut self, cx: &mut Context<'_, Self>) {
        if self.spawn.ssh_host.is_some() {
            return;
        }
        self.containers_scanning = true;
        let root = self
            .local_project_root
            .clone()
            .or_else(|| self.project_root.clone());
        cx.spawn(async move |this, cx| {
            let probe = cx
                .background_executor()
                .spawn(async move { operate::list_containers(root.as_deref()) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.note_containers(&probe);
                view.containers = Some(probe);
                view.containers_at = Some(SystemTime::now());
                view.containers_scanning = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Start, stop or restart one container the user picked.
    fn container_action(
        &mut self,
        action: ContainerAction,
        id: String,
        name: String,
        cx: &mut Context<'_, Self>,
    ) {
        if self.container_busy.is_some() {
            return;
        }
        self.container_busy = Some(id.clone());
        self.container_note = Some(format!("{} {name}…", action.progressive()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { operate::run_container_action(action, &id) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.container_busy = None;
                match result {
                    Ok(()) => {
                        view.container_note = None;
                    }
                    Err(error) => {
                        view.activity.record(
                            Source::Container,
                            Level::Error,
                            format!("{name}: {} failed: {error}", action.verb()),
                        );
                        view.container_note =
                            Some(format!("{} {name} failed: {error}", action.verb()));
                    }
                }
                view.refresh_containers(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn containers_section(&self, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        let t = &self.theme;
        let mut out: Vec<gpui::AnyElement> = Vec::new();
        let title = match self.container_runtime {
            Some(runtime) => format!("CONTAINERS ({runtime})"),
            None => "CONTAINERS".to_owned(),
        };
        out.push(self.operate_section_dyn(title).into_any_element());
        if self.spawn.ssh_host.is_some() {
            out.push(
                self.muted_line("Containers are read from this machine, not the remote host."),
            );
            return out;
        }
        let files = self
            .local_project_root
            .as_deref()
            .map(operate::container_files)
            .unwrap_or_default();
        match &self.containers {
            None if self.container_runtime.is_none() => {
                out.push(self.muted_line("No docker or podman found on PATH."));
            }
            None => out.push(self.muted_line("Looking for containers…")),
            Some(ContainerProbe::NoRuntime) => {
                out.push(self.muted_line("No docker or podman found on PATH."));
            }
            Some(ContainerProbe::Unreachable { runtime, message }) => {
                out.push(self.muted_line(&format!("{runtime} did not answer: {message}")));
            }
            Some(ContainerProbe::Listed {
                runtime,
                containers,
            }) => {
                let runtime = *runtime;
                let mine = containers.iter().filter(|c| c.in_project).count();
                if containers.is_empty() {
                    out.push(self.muted_line("No containers exist."));
                } else if mine == 0 {
                    out.push(self.muted_line("No container was started from this project."));
                }
                const OTHERS: usize = 6;
                let others_hidden = containers
                    .iter()
                    .filter(|c| !c.in_project)
                    .count()
                    .saturating_sub(OTHERS);
                let mut other_seen = 0;
                for (n, c) in containers.iter().enumerate() {
                    if !c.in_project {
                        other_seen += 1;
                        if other_seen > OTHERS {
                            continue;
                        }
                    }
                    let color = match c.state {
                        ContainerState::Running => t.success,
                        ContainerState::Exited => t.text_faint,
                        ContainerState::Restarting | ContainerState::Paused => t.warning,
                        _ => t.text_muted,
                    };
                    let safe = operate::is_safe_container_id(&c.id);
                    let busy = self.container_busy.as_deref() == Some(c.id.as_str());
                    let running = c.state == ContainerState::Running;
                    let (id, name) = (c.id.clone(), c.name.clone());
                    let label = match (&c.compose_project, &c.compose_service) {
                        (Some(project), Some(service)) => format!("{project}/{service}"),
                        _ => c.name.clone(),
                    };
                    let detail = if c.ports.is_empty() {
                        format!("{} · {}", c.image, c.status)
                    } else {
                        format!("{} · {} · {}", c.image, c.status, c.ports)
                    };
                    let act = |suffix: &'static str, text: &'static str, color: Rgb| {
                        div()
                            .id(SharedString::from(format!("container-{suffix}-{n}")))
                            .px(px(tokens::space::XS))
                            .rounded(px(tokens::radius::SM))
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(color))
                            .cursor_pointer()
                            .hover(|style| style.bg(with_alpha(hsla(t.text), 0.08)))
                            .child(text)
                    };
                    let mut row = div()
                        .id(("container-row", n))
                        .min_h(px(24.0))
                        .px(px(tokens::space::SM))
                        .flex()
                        .items_center()
                        .gap(px(tokens::space::SM))
                        .rounded(px(tokens::radius::SM))
                        .text_size(px(tokens::text::SM))
                        .text_color(hsla(if c.in_project { t.text } else { t.text_faint }))
                        .hover(|style| style.bg(with_alpha(hsla(t.text), 0.05)))
                        .child(div().size(px(6.0)).rounded_full().bg(hsla(color)))
                        .child(
                            div()
                                .w(px(130.0))
                                .flex_none()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(SharedString::from(label)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child(SharedString::from(detail)),
                        );
                    if safe && !busy {
                        let (logs_id, logs_name) = (id.clone(), name.clone());
                        row = row.child(act("logs", "logs", t.accent).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.start_managed(
                                    format!("logs: {logs_name}"),
                                    format!("{runtime} logs -f --tail 200 {logs_id}"),
                                    false,
                                    cx,
                                );
                            },
                        )));
                        if running {
                            let (shell_id, shell_name) = (id.clone(), name.clone());
                            row = row.child(act("shell", "shell", t.accent).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.start_managed(
                                        format!("shell: {shell_name}"),
                                        format!("{runtime} exec -it {shell_id} sh"),
                                        false,
                                        cx,
                                    );
                                },
                            )));
                            for (suffix, text, action, tone) in [
                                ("stop", "stop", ContainerAction::Stop, t.warning),
                                ("restart", "restart", ContainerAction::Restart, t.accent),
                            ] {
                                let (aid, aname) = (id.clone(), name.clone());
                                row = row.child(act(suffix, text, tone).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.container_action(
                                            action,
                                            aid.clone(),
                                            aname.clone(),
                                            cx,
                                        );
                                    },
                                )));
                            }
                        } else {
                            let (aid, aname) = (id.clone(), name.clone());
                            row = row.child(act("start", "start", t.accent).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.container_action(
                                        ContainerAction::Start,
                                        aid.clone(),
                                        aname.clone(),
                                        cx,
                                    );
                                },
                            )));
                        }
                    }
                    out.push(row.into_any_element());
                }
                if others_hidden > 0 {
                    out.push(
                        self.muted_line(&format!("+{others_hidden} more outside this project")),
                    );
                }
            }
        }
        if let Some(note) = &self.container_note {
            out.push(self.muted_line(note));
        }
        if !files.is_empty() {
            out.push(self.muted_line(&format!("Project defines: {}.", files.join(", "))));
        }
        out
    }

    fn operate_section_dyn(&self, title: String) -> gpui::Div {
        div()
            .pt(px(tokens::space::SM))
            .pb(px(2.0))
            .text_size(px(tokens::text::XS))
            .text_color(hsla(self.theme.text_faint))
            .child(SharedString::from(title))
    }
}
