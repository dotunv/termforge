//! Operate workspace areas: where work runs, what is running, and what is
//! listening. Everything shown is observed; where the answer cannot be known
//! (a remote host, a process owned by another user, a lost daemon) the area
//! says so.

use std::time::{Duration, SystemTime};

use super::*;
use crate::operate::{self, PortProbe, Support};

/// Ports shown from outside the project before the rest are summarised.
const OTHER_PORTS_SHOWN: usize = 8;
const PORT_REFRESH: Duration = Duration::from_secs(3);

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
        let capabilities = operate::capabilities(remote.as_deref());
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
