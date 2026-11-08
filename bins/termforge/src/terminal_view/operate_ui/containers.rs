//! Containers in the Services area, and the actions on them.

use std::time::SystemTime;

use super::*;
use crate::activity::{Level, Source};
use crate::operate::{self, ContainerAction, ContainerProbe, ContainerState};

impl TerminalView {
    pub(in crate::terminal_view) fn refresh_containers_if_stale(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) {
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

    pub(in crate::terminal_view) fn refresh_containers(&mut self, cx: &mut Context<'_, Self>) {
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
    pub(super) fn container_action(
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

    pub(super) fn containers_section(&self, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        let t = &self.theme;
        let mut out: Vec<gpui::AnyElement> = Vec::new();
        let title = match self.container_runtime {
            Some(runtime) => format!("Containers · {runtime}"),
            None => "Containers".to_owned(),
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
}
