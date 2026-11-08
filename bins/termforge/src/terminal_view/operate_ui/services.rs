//! The Services area: sessions, services, launchers and listening ports.

use super::*;
use crate::operate::PortProbe;

impl TerminalView {
    /// Sessions and the processes TermForge started, with observed state and
    /// the actions it can really perform, followed by listening ports.
    pub(in crate::terminal_view) fn render_area_services(
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
            .gap(px(tokens::space::SM))
            .child(self.editor_intro(
                "Services & sessions",
                "Open a shell, follow output, or manage a service started here.",
            ))
            .child(self.operate_section("Sessions"))
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
                    .min_h(px(48.0))
                    .px(px(tokens::space::SM))
                    .py(px(tokens::space::SM))
                    .border_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.bg_app))
                    .flex()
                    .flex_wrap()
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
            .child(self.operate_section("Launch a service"))
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
                    .child(self.operate_section("Listening ports · TCP"))
                    .child(
                        div()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(SharedString::from(scan_note)),
                    ),
            )
            .children(ports_body)
    }
}
