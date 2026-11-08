//! The Environment area: identity, connection and capabilities.

use super::*;
use crate::operate::{self, Support};

impl TerminalView {
    /// Clear identity first, then what is known about connection and what
    /// this environment can and cannot do.
    pub(in crate::terminal_view) fn render_area_environment(
        &self,
        id: AreaId,
        cx: &mut Context<'_, Self>,
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
        let unavailable = capabilities
            .iter()
            .filter(|c| matches!(c.support, Support::Unavailable(_)))
            .count();

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
            .max_w(px(720.0))
            .overflow_y_scroll()
            .p(px(tokens::space::MD))
            .bg(hsla(t.bg_panel))
            .flex()
            .flex_col()
            .gap(px(tokens::space::SM))
            .child(self.editor_intro(
                "Environment",
                if remote.is_some() {
                    "Remote shell access. Project files and Git remain local."
                } else {
                    "Project identity, session host and available capabilities."
                },
            ))
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
            .child(self.operate_section("Project identity"))
            .child(self.operate_row("Project", self.project_name()))
            .child(self.operate_row("Path", project))
            .child(self.operate_row("Branch", branch))
            .child(self.operate_row("Context", context))
            .child(self.operate_section("Session host"))
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
            .child(self.operate_section("Sessions"))
            .children(hosts.into_iter().map(|(host, count)| {
                self.operate_row(
                    "",
                    format!(
                        "{host} · {count} session{}",
                        if count == 1 { "" } else { "s" }
                    ),
                )
            }))
            .child(self.operate_section("Capabilities"))
            .child(self.inspector_action(
                "environment-capabilities-toggle",
                if self.environment_show_unavailable {
                    "Hide unavailable capabilities"
                } else {
                    "Show unavailable capabilities"
                },
                cx.listener(|this, _, _, cx| {
                    this.environment_show_unavailable = !this.environment_show_unavailable;
                    cx.notify();
                }),
            ))
            .child(self.muted_line(&format!(
                "{unavailable} capabilities unavailable in this environment"
            )))
            .children(
                capabilities
                    .into_iter()
                    .filter(|capability| {
                        self.environment_show_unavailable
                            || matches!(capability.support, Support::Available)
                    })
                    .map(|capability| {
                        let (mark, color, note) = match capability.support {
                            Support::Available => ("✓", t.success, String::new()),
                            Support::Unavailable(reason) => {
                                ("✕", t.text_faint, format!(" — {reason}"))
                            }
                        };
                        div()
                            .flex()
                            .gap(px(tokens::space::SM))
                            .text_size(px(tokens::text::SM))
                            .child(div().w(px(12.0)).text_color(hsla(color)).child(mark))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_color(hsla(if mark == "✓" {
                                        t.text
                                    } else {
                                        t.text_muted
                                    }))
                                    .child(SharedString::from(format!(
                                        "{}{note}",
                                        capability.name
                                    ))),
                            )
                    }),
            )
            .child(self.operate_section("Connection history"))
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
}
