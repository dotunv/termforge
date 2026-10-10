//! Project tasks in the sidebar: list, create, run, finish and delete, plus
//! suggestions for commands worth saving.
//!
//! Tasks live in the daemon (`forged`). Every daemon call here runs on a
//! background thread so a slow or absent daemon never stalls the UI.

use tf_proto::{TaskInfo, TaskState};

use super::*;
use crate::suggest::{self, Frequency, Suggestion};

/// How many times a command must be entered before it is suggested.
const FREQUENT_AFTER: u32 = 3;
/// Most suggestions shown at once.
const MAX_SUGGESTIONS: usize = 5;

/// Which field of the new-task form has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskField {
    Title,
    Command,
}

#[derive(Debug)]
pub(super) struct TaskForm {
    title: String,
    command: String,
    field: TaskField,
}

#[derive(Debug, Default)]
pub(super) struct TasksState {
    items: Vec<TaskInfo>,
    show_done: bool,
    form: Option<TaskForm>,
    /// Project-detected common commands.
    suggestions: Vec<Suggestion>,
    typed: Frequency,
}

impl TasksState {
    pub(super) fn for_project(root: Option<&std::path::Path>) -> Self {
        Self {
            suggestions: root.map(suggest::for_project).unwrap_or_default(),
            ..Self::default()
        }
    }
}

fn state_glyph(state: TaskState) -> &'static str {
    match state {
        TaskState::Planned => "○",
        TaskState::Active => "◐",
        TaskState::Blocked => "▲",
        TaskState::Done => "●",
    }
}

impl TerminalView {
    // ---- daemon access ---------------------------------------------------

    pub(super) fn refresh_tasks(&mut self, cx: &mut Context<'_, Self>) {
        let Some(root) = self.local_project_root.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { remote_session::list_tasks(root) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(items) => view.tasks.items = items,
                    Err(error) => tracing::debug!("could not list tasks: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Run a daemon mutation off the UI thread, then reload the list.
    fn mutate_tasks(
        &mut self,
        job: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
        cx: &mut Context<'_, Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { job() }).await;
            if let Err(error) = &result {
                tracing::warn!("task update failed: {error:#}");
            }
            let _ = this.update(cx, |view, cx| view.refresh_tasks(cx));
        })
        .detach();
    }

    // ---- actions -----------------------------------------------------------

    pub(super) fn open_task_form(
        &mut self,
        title: &str,
        command: &str,
        cx: &mut Context<'_, Self>,
    ) {
        self.palette = None;
        self.search = None;
        self.tasks.form = Some(TaskForm {
            title: title.to_owned(),
            command: command.to_owned(),
            field: if title.is_empty() {
                TaskField::Title
            } else {
                TaskField::Command
            },
        });
        cx.notify();
    }

    fn save_task(&mut self, title: String, command: String, cx: &mut Context<'_, Self>) {
        let Some(root) = self.local_project_root.clone() else {
            return;
        };
        let command = command.trim().to_owned();
        let title = match title.trim() {
            "" => command.clone(),
            title => title.to_owned(),
        };
        if title.is_empty() {
            return;
        }
        let command = (!command.is_empty()).then_some(command);
        self.mutate_tasks(
            move || remote_session::create_task(root, title, command),
            cx,
        );
    }

    fn submit_task_form(&mut self, cx: &mut Context<'_, Self>) {
        let Some(form) = self.tasks.form.take() else {
            return;
        };
        self.save_task(form.title, form.command, cx);
        cx.notify();
    }

    /// Planned → Active → Done → Planned; a blocked task resumes as Active.
    fn cycle_task(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(task) = self.tasks.items.get(index) else {
            return;
        };
        let next = match task.state {
            TaskState::Planned | TaskState::Blocked => TaskState::Active,
            TaskState::Active => TaskState::Done,
            TaskState::Done => TaskState::Planned,
        };
        let id = task.id;
        self.mutate_tasks(move || remote_session::set_task_state(id, next), cx);
    }

    fn delete_task(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(task) = self.tasks.items.get(index) else {
            return;
        };
        let id = task.id;
        self.mutate_tasks(move || remote_session::delete_task(id), cx);
    }

    /// Type a task's command into the active terminal and mark it active.
    fn run_task(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(task) = self.tasks.items.get(index) else {
            return;
        };
        let Some(command) = task.command.clone() else {
            return;
        };
        if self.exited().is_some() {
            return;
        }
        // Enter is what a user would press; a newline would not submit in
        // every shell.
        self.write(format!("{command}\r").as_bytes());
        if matches!(task.state, TaskState::Planned | TaskState::Done) {
            let id = task.id;
            self.mutate_tasks(
                move || remote_session::set_task_state(id, TaskState::Active),
                cx,
            );
        }
    }

    /// Called as Enter is pressed at a prompt: remember the command so one
    /// that keeps coming back can be offered as a task.
    pub(super) fn note_enter(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let line = session.with(|p| {
            let engine = p.engine();
            let snapshot = engine.snapshot();
            if snapshot.modes.alt_screen {
                return None;
            }
            // Only a prompt being typed at; not a running program's input.
            let at_prompt = p
                .blocks()
                .iter()
                .last()
                .is_none_or(|block| block.state == tf_session::BlockState::Prompt);
            at_prompt.then(|| snapshot.row_text(usize::from(snapshot.cursor.row)))
        });
        if let Some(command) = line.as_deref().and_then(suggest::extract_command) {
            self.tasks.typed.record(&command);
        }
    }

    // ---- form input ----------------------------------------------------------

    /// Handle a key while the new-task form is open. Returns whether it was
    /// consumed.
    pub(super) fn task_form_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<'_, Self>) -> bool {
        if self.tasks.form.is_none() {
            return false;
        }
        let key = ev.keystroke.key.as_str();
        let modifiers = ev.keystroke.modifiers;
        match key {
            "escape" => self.tasks.form = None,
            "enter" => {
                self.submit_task_form(cx);
                return true;
            }
            "tab" => {
                if let Some(form) = &mut self.tasks.form {
                    form.field = match form.field {
                        TaskField::Title => TaskField::Command,
                        TaskField::Command => TaskField::Title,
                    };
                }
            }
            "backspace" => {
                if let Some(form) = &mut self.tasks.form {
                    form.focused_mut().pop();
                }
            }
            "v" if modifiers.control => {
                let pasted = cx.read_from_clipboard().and_then(|item| item.text());
                if let (Some(form), Some(text)) = (&mut self.tasks.form, pasted) {
                    let line = text.lines().next().unwrap_or_default();
                    form.focused_mut()
                        .extend(line.chars().filter(|c| !c.is_control()));
                }
            }
            _ if !modifiers.control && !modifiers.alt => {
                if let (Some(form), Some(text)) =
                    (&mut self.tasks.form, ev.keystroke.key_char.as_deref())
                {
                    if !text.chars().any(char::is_control) {
                        form.focused_mut().push_str(text);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
        true
    }

    // ---- rendering ---------------------------------------------------------

    /// Everything to suggest right now: commands run repeatedly first, then
    /// common ones for this project, minus commands that are already tasks.
    fn current_suggestions(&self) -> Vec<(String, String, String)> {
        let existing = |command: &str| {
            self.tasks
                .items
                .iter()
                .any(|task| task.command.as_deref() == Some(command))
        };
        let mut out: Vec<(String, String, String)> = self
            .tasks
            .typed
            .frequent(FREQUENT_AFTER, existing)
            .into_iter()
            .map(|(command, count)| {
                (
                    command.clone(),
                    command,
                    format!("You ran this {count} times"),
                )
            })
            .collect();
        for suggestion in &self.tasks.suggestions {
            if !existing(&suggestion.command)
                && !out.iter().any(|(_, c, _)| *c == suggestion.command)
            {
                out.push((
                    suggestion.title.clone(),
                    suggestion.command.clone(),
                    suggestion.reason.clone(),
                ));
            }
        }
        out.truncate(MAX_SUGGESTIONS);
        out
    }

    fn task_row(
        &self,
        index: usize,
        task: &TaskInfo,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let done = task.state == TaskState::Done;
        let group = SharedString::from(format!("task-row-{index}"));
        let has_command = task.command.is_some();
        let glyph_color = match task.state {
            TaskState::Done => t.success,
            TaskState::Active => t.accent,
            TaskState::Blocked => t.warning,
            TaskState::Planned => t.text_faint,
        };
        div()
            .id(("task-row", index))
            .group(group.clone())
            .px(px(tokens::space::SM))
            .py(px(5.0))
            .flex()
            .items_start()
            .gap(px(tokens::space::SM))
            .rounded(px(tokens::radius::MD))
            .hover(|style| style.bg(hsla(t.bg_hover)))
            .child(
                div()
                    .id(("task-state", index))
                    .mt(px(1.0))
                    .size(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .text_color(hsla(glyph_color))
                    .hover(|style| style.bg(hsla(t.bg_selected)))
                    .on_click(cx.listener(move |this, _, _, cx| this.cycle_task(index, cx)))
                    .child(state_glyph(task.state)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(hsla(if done { t.text_faint } else { t.text }))
                            .child(SharedString::from(task.title.clone())),
                    )
                    .children(task.command.clone().map(|command| {
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(SharedString::from(format!("$ {command}")))
                    })),
            )
            .children(has_command.then(|| {
                div()
                    .id(("task-run", index))
                    .flex_none()
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .text_color(hsla(t.accent))
                    .invisible()
                    .group_hover(group.clone(), |style| style.visible())
                    .hover(|style| style.bg(hsla(t.bg_selected)))
                    .on_click(cx.listener(move |this, _, _, cx| this.run_task(index, cx)))
                    .child("▶")
            }))
            .child(
                div()
                    .id(("task-delete", index))
                    .flex_none()
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(tokens::radius::SM))
                    .text_color(hsla(t.text_faint))
                    .invisible()
                    .group_hover(group, |style| style.visible())
                    .hover(|style| style.bg(hsla(t.bg_selected)).text_color(hsla(t.text)))
                    .on_click(cx.listener(move |this, _, _, cx| this.delete_task(index, cx)))
                    .child("×"),
            )
    }

    fn suggestion_row(
        &self,
        index: usize,
        title: String,
        command: String,
        reason: String,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(("task-suggestion", index))
            .px(px(tokens::space::SM))
            .py(px(5.0))
            .flex()
            .items_start()
            .gap(px(tokens::space::SM))
            .rounded(px(tokens::radius::MD))
            .cursor_pointer()
            .hover(|style| style.bg(hsla(t.bg_hover)))
            .on_click({
                let title = title.clone();
                let command = command.clone();
                cx.listener(move |this, _, _, cx| this.open_task_form(&title, &command, cx))
            })
            .child(
                div()
                    .mt(px(1.0))
                    .size(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(hsla(t.accent))
                    .child("+"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(hsla(t.text_muted))
                            .child(SharedString::from(command)),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(SharedString::from(reason)),
                    ),
            )
    }

    /// The TASKS section: tasks, then suggestions.
    pub(super) fn tasks_section(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let t = &self.theme;
        let open = !self.collapsed_sections.contains(&SidebarSection::Tasks);
        let view = cx.entity();
        let add: ClickHandler = Box::new(move |_, _, cx| {
            view.update(cx, |this, cx| this.open_task_form("", "", cx));
        });
        let show_done = self.tasks.show_done;
        let visible: Vec<usize> = self
            .tasks
            .items
            .iter()
            .enumerate()
            .filter(|(_, task)| show_done || task.state != TaskState::Done)
            .map(|(index, _)| index)
            .collect();
        let hidden_done = self
            .tasks
            .items
            .iter()
            .filter(|task| task.state == TaskState::Done)
            .count();
        let rows: Vec<_> = visible
            .into_iter()
            .map(|index| self.task_row(index, &self.tasks.items[index], cx))
            .collect();
        let suggestions: Vec<_> = self
            .current_suggestions()
            .into_iter()
            .enumerate()
            .map(|(index, (title, command, reason))| {
                self.suggestion_row(index, title, command, reason, cx)
            })
            .collect();
        let no_rows = rows.is_empty();
        let has_suggestions = !suggestions.is_empty();
        let caption = |text: &'static str| {
            div()
                .px(px(tokens::space::SM))
                .pt(px(tokens::space::SM))
                .pb(px(2.0))
                .text_size(px(tokens::text::XS))
                .text_color(hsla(t.text_faint))
                .child(text)
        };
        div()
            .flex()
            .flex_col()
            .child(self.section_header(
                SidebarSection::Tasks,
                "section-tasks",
                "TASKS",
                Some(("section-tasks-add", "+", add)),
                cx,
            ))
            .when(open, |d| {
                d.when(no_rows, |d| {
                    d.child(
                        div()
                            .px(px(tokens::space::SM))
                            .py(px(4.0))
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .child(if self.local_project_root.is_some() {
                                "No tasks yet. Add one with +, or pick a suggestion."
                            } else {
                                "Tasks need a project folder."
                            }),
                    )
                })
                .child(div().flex().flex_col().gap(px(2.0)).children(rows))
                .when(hidden_done > 0, |d| {
                    d.child(
                        div()
                            .id("tasks-toggle-done")
                            .px(px(tokens::space::SM))
                            .py(px(4.0))
                            .text_size(px(tokens::text::XS))
                            .text_color(hsla(t.text_faint))
                            .hover(|style| style.text_color(hsla(t.text)))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tasks.show_done = !this.tasks.show_done;
                                cx.notify();
                            }))
                            .child(SharedString::from(if show_done {
                                "Hide finished".to_owned()
                            } else {
                                format!("Show {hidden_done} finished")
                            })),
                    )
                })
                .when(has_suggestions, |d| {
                    d.child(caption("SUGGESTED"))
                        .child(div().flex().flex_col().gap(px(2.0)).children(suggestions))
                })
            })
    }

    /// The new-task form, shown over the terminal.
    pub(super) fn task_form_overlay(&self, cx: &mut Context<'_, Self>) -> Option<impl IntoElement> {
        let form = self.tasks.form.as_ref()?;
        let t = &self.theme;
        let field = |label: &'static str, value: &str, placeholder: &'static str, focused: bool| {
            let empty = value.is_empty();
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(t.text_faint))
                        .child(label),
                )
                .child(
                    div()
                        .h(px(34.0))
                        .px(px(tokens::space::MD))
                        .flex()
                        .items_center()
                        .rounded(px(tokens::radius::MD))
                        .bg(hsla(t.bg_app))
                        .border_1()
                        .border_color(hsla(if focused { t.accent } else { t.border_strong }))
                        .text_color(hsla(if empty { t.text_faint } else { t.text }))
                        .child(SharedString::from(if empty {
                            placeholder.to_owned()
                        } else if focused {
                            format!("{value}▏")
                        } else {
                            value.to_owned()
                        })),
                )
        };
        let can_save = !form.title.trim().is_empty() || !form.command.trim().is_empty();
        Some(
            div()
                .id("task-form-scrim")
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .items_start()
                .justify_center()
                .bg(gpui::black().opacity(0.4))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tasks.form = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("task-form")
                        .mt(px(64.0))
                        .w(px(520.0))
                        .max_w_full()
                        .p(px(tokens::space::LG))
                        .flex()
                        .flex_col()
                        .gap(px(tokens::space::MD))
                        .rounded(px(tokens::radius::XL))
                        .bg(hsla(t.bg_elevated))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_size(px(tokens::text::LG))
                                .text_color(hsla(t.text))
                                .child("New task"),
                        )
                        .child(field(
                            "TITLE",
                            &form.title,
                            "What needs doing?",
                            form.field == TaskField::Title,
                        ))
                        .child(field(
                            "COMMAND (OPTIONAL)",
                            &form.command,
                            "e.g. cargo test --workspace",
                            form.field == TaskField::Command,
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(t.text_faint))
                                .child("Tab switch field   Enter save   Esc cancel")
                                .child(
                                    div()
                                        .id("task-form-save")
                                        .h(px(30.0))
                                        .px(px(tokens::space::LG))
                                        .flex()
                                        .items_center()
                                        .rounded(px(tokens::radius::MD))
                                        .bg(hsla(if can_save { t.accent } else { t.bg_hover }))
                                        .text_color(hsla(if can_save {
                                            t.accent_text
                                        } else {
                                            t.text_faint
                                        }))
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.submit_task_form(cx);
                                        }))
                                        .child("Create task"),
                                ),
                        ),
                ),
        )
    }
}

impl TaskForm {
    fn focused_mut(&mut self) -> &mut String {
        match self.field {
            TaskField::Title => &mut self.title,
            TaskField::Command => &mut self.command,
        }
    }
}
