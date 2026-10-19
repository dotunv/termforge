//! Client/account profile editor and Accounts settings page.

use std::path::PathBuf;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountField {
    Id,
    Name,
    Color,
    GitName,
    GitEmail,
    GithubHost,
    GhConfigDir,
    GitConfigGlobal,
}

impl AccountField {
    const ALL: [Self; 8] = [
        Self::Id,
        Self::Name,
        Self::Color,
        Self::GitName,
        Self::GitEmail,
        Self::GithubHost,
        Self::GhConfigDir,
        Self::GitConfigGlobal,
    ];

    fn next(self, backwards: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|field| *field == self)
            .unwrap_or(0);
        let next = if backwards {
            (index + Self::ALL.len() - 1) % Self::ALL.len()
        } else {
            (index + 1) % Self::ALL.len()
        };
        Self::ALL[next]
    }

    fn is_advanced(self) -> bool {
        matches!(
            self,
            Self::GithubHost | Self::GhConfigDir | Self::GitConfigGlobal
        )
    }
}

#[derive(Debug, Clone)]
struct AccountDraft {
    id: String,
    name: String,
    color: String,
    git_name: String,
    git_email: String,
    github_host: String,
    gh_config_dir: String,
    git_config_global: String,
}

impl AccountDraft {
    fn from_config(context: &ContextConfig) -> Self {
        Self {
            id: context.id.clone(),
            name: context.name.clone(),
            color: context.color.clone(),
            git_name: context.git_name.clone(),
            git_email: context.git_email.clone(),
            github_host: context.github_host.clone(),
            gh_config_dir: context
                .gh_config_dir
                .as_deref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
            git_config_global: context
                .git_config_global
                .as_deref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }

    fn into_config(self) -> ContextConfig {
        ContextConfig {
            id: self.id.trim().to_owned(),
            name: self.name.trim().to_owned(),
            color: self.color.trim().to_owned(),
            git_name: self.git_name.trim().to_owned(),
            git_email: self.git_email.trim().to_owned(),
            github_host: self.github_host.trim().to_owned(),
            gh_config_dir: path_or_none(&self.gh_config_dir),
            git_config_global: path_or_none(&self.git_config_global),
        }
    }

    fn value(&self, field: AccountField) -> &str {
        match field {
            AccountField::Id => &self.id,
            AccountField::Name => &self.name,
            AccountField::Color => &self.color,
            AccountField::GitName => &self.git_name,
            AccountField::GitEmail => &self.git_email,
            AccountField::GithubHost => &self.github_host,
            AccountField::GhConfigDir => &self.gh_config_dir,
            AccountField::GitConfigGlobal => &self.git_config_global,
        }
    }

    fn value_mut(&mut self, field: AccountField) -> &mut String {
        match field {
            AccountField::Id => &mut self.id,
            AccountField::Name => &mut self.name,
            AccountField::Color => &mut self.color,
            AccountField::GitName => &mut self.git_name,
            AccountField::GitEmail => &mut self.git_email,
            AccountField::GithubHost => &mut self.github_host,
            AccountField::GhConfigDir => &mut self.gh_config_dir,
            AccountField::GitConfigGlobal => &mut self.git_config_global,
        }
    }
}

fn path_or_none(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn account_validation_error(context: &ContextConfig) -> Option<&'static str> {
    if context.id.is_empty() {
        return Some("Give this account a short ID, such as `client-a`.");
    }
    if !context
        .id
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Some("Account IDs use lowercase letters, numbers, and hyphens only.");
    }
    if context.name.is_empty() {
        return Some("Give this account a name so it is easy to recognize.");
    }
    None
}

#[derive(Debug)]
pub(super) struct AccountEditor {
    original_id: Option<String>,
    draft: AccountDraft,
    field: AccountField,
    show_advanced: bool,
    error: Option<String>,
}

impl TerminalView {
    pub(super) fn settings_accounts(&self, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        let t = &self.theme;
        let active = self.context_id.as_deref();
        let mut rows: Vec<gpui::AnyElement> = self
            .appearance
            .config
            .contexts
            .iter()
            .enumerate()
            .map(|(index, context)| {
                let id = context.id.clone();
                let edit_id = id.clone();
                let duplicate_id = id.clone();
                let delete_id = id.clone();
                let selected = active == Some(id.as_str());
                let color = Rgb::from_hex(&context.color).unwrap_or(t.accent);
                let detail = [
                    (!context.git_email.is_empty()).then_some(context.git_email.as_str()),
                    (!context.github_host.is_empty()).then_some(context.github_host.as_str()),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                let confirming = self.pending_context_delete.as_deref() == Some(id.as_str());
                div()
                    .px(px(tokens::space::MD))
                    .py(px(tokens::space::MD))
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::MD))
                    .child(div().size(px(10.0)).rounded_full().bg(hsla(color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_color(hsla(t.text))
                                    .child(SharedString::from(context.name.clone())),
                            )
                            .child(
                                div()
                                    .text_size(px(tokens::text::XS))
                                    .text_color(hsla(t.text_faint))
                                    .child(SharedString::from(if detail.is_empty() {
                                        format!("{} · no identity constraints", context.id)
                                    } else {
                                        format!("{} · {detail}", context.id)
                                    })),
                            ),
                    )
                    .when(selected, |d| {
                        d.child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded_full()
                                .bg(with_alpha(hsla(color), 0.18))
                                .text_size(px(tokens::text::XS))
                                .text_color(hsla(color))
                                .child("CURRENT"),
                        )
                    })
                    .when(!selected, |d| {
                        d.child(self.button(
                            SharedString::from(format!("account-use-{index}")),
                            "Use",
                            cx.listener(move |this, _, _, cx| this.set_context(&id, cx)),
                        ))
                    })
                    .child(self.button(
                        SharedString::from(format!("account-edit-{index}")),
                        "Edit",
                        cx.listener(move |this, _, _, cx| {
                            this.open_account_editor(Some(&edit_id), cx)
                        }),
                    ))
                    .child(self.button(
                        SharedString::from(format!("account-copy-{index}")),
                        "Duplicate",
                        cx.listener(move |this, _, _, cx| {
                            this.duplicate_context(&duplicate_id, cx)
                        }),
                    ))
                    .child(self.button(
                        SharedString::from(format!("account-delete-{index}")),
                        if confirming { "Confirm" } else { "Delete" },
                        cx.listener(move |this, _, _, cx| {
                            this.request_context_delete(&delete_id, cx)
                        }),
                    ))
                    .into_any_element()
            })
            .collect();

        if !self.identity_check.warnings.is_empty() {
            rows.push(
                div()
                    .px(px(tokens::space::MD))
                    .py(px(tokens::space::MD))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .text_size(px(tokens::text::SM))
                    .text_color(hsla(t.warning))
                    .children(self.identity_check.warnings.iter().map(|warning| {
                        div().child(SharedString::from(format!("Identity mismatch: {warning}")))
                    }))
                    .into_any_element(),
            );
        }

        vec![
            self.settings_section_title("ACCOUNTS").into_any_element(),
            div()
                .mb(px(tokens::space::MD))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .max_w(px(560.0))
                        .text_size(px(tokens::text::SM))
                        .text_color(hsla(t.text_muted))
                        .child("Use the right Git and GitHub account for each project."),
                )
                .child(self.button(
                    "account-add",
                    "+ Add account",
                    cx.listener(|this, _, _, cx| this.open_account_editor(None, cx)),
                ))
                .into_any_element(),
            self.settings_card(rows),
        ]
    }

    fn open_account_editor(&mut self, id: Option<&str>, cx: &mut Context<'_, Self>) {
        let (original_id, draft, field) = match id.and_then(|id| {
            self.appearance
                .config
                .contexts
                .iter()
                .find(|context| context.id == id)
        }) {
            Some(context) => (
                Some(context.id.clone()),
                AccountDraft::from_config(context),
                AccountField::Name,
            ),
            None => {
                let number = self.appearance.config.contexts.len() + 1;
                (
                    None,
                    AccountDraft {
                        id: format!("context-{number}"),
                        name: String::new(),
                        color: "#5e6ad2".into(),
                        git_name: String::new(),
                        git_email: String::new(),
                        github_host: "github.com".into(),
                        gh_config_dir: String::new(),
                        git_config_global: String::new(),
                    },
                    AccountField::Name,
                )
            }
        };
        self.pending_context_delete = None;
        self.account_editor = Some(AccountEditor {
            original_id,
            draft,
            field,
            show_advanced: false,
            error: None,
        });
        cx.notify();
    }

    fn duplicate_context(&mut self, id: &str, cx: &mut Context<'_, Self>) {
        let Some(context) = self
            .appearance
            .config
            .contexts
            .iter()
            .find(|context| context.id == id)
            .cloned()
        else {
            return;
        };
        let mut draft = AccountDraft::from_config(&context);
        let base = format!("{}-copy", context.id);
        let mut candidate = base.clone();
        let mut suffix = 2;
        while self
            .appearance
            .config
            .contexts
            .iter()
            .any(|context| context.id == candidate)
        {
            candidate = format!("{base}-{suffix}");
            suffix += 1;
        }
        draft.id = candidate;
        draft.name = format!("{} Copy", context.name);
        self.account_editor = Some(AccountEditor {
            original_id: None,
            draft,
            field: AccountField::Name,
            show_advanced: false,
            error: None,
        });
        cx.notify();
    }

    fn request_context_delete(&mut self, id: &str, cx: &mut Context<'_, Self>) {
        if self.appearance.config.contexts.len() <= 1 {
            self.appearance.save_error = Some("Keep at least one account".into());
            cx.notify();
            return;
        }
        if self.pending_context_delete.as_deref() != Some(id) {
            self.pending_context_delete = Some(id.to_owned());
            cx.notify();
            return;
        }
        self.pending_context_delete = None;
        self.appearance
            .config
            .contexts
            .retain(|context| context.id != id);
        self.appearance.save();
        if self.context_id.as_deref() == Some(id) {
            if let Some(next) = self
                .appearance
                .config
                .contexts
                .first()
                .map(|c| c.id.clone())
            {
                self.set_context(&next, cx);
            }
        } else {
            cx.notify();
        }
    }

    pub(super) fn account_editor_key(
        &mut self,
        ev: &KeyDownEvent,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(editor) = &mut self.account_editor else {
            return false;
        };
        let key = ev.keystroke.key.as_str();
        let modifiers = ev.keystroke.modifiers;
        match key {
            "escape" => self.account_editor = None,
            "enter" => {
                self.submit_account_editor(cx);
                return true;
            }
            "tab" => {
                editor.field = editor.field.next(modifiers.shift);
                if editor.original_id.is_some() && editor.field == AccountField::Id {
                    editor.field = editor.field.next(modifiers.shift);
                }
                while !editor.show_advanced && editor.field.is_advanced() {
                    editor.field = editor.field.next(modifiers.shift);
                    if editor.original_id.is_some() && editor.field == AccountField::Id {
                        editor.field = editor.field.next(modifiers.shift);
                    }
                }
            }
            "backspace" => {
                if editor.original_id.is_none() || editor.field != AccountField::Id {
                    editor.draft.value_mut(editor.field).pop();
                    editor.error = None;
                }
            }
            "v" if modifiers.control => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    let line = text.lines().next().unwrap_or_default();
                    if editor.original_id.is_none() || editor.field != AccountField::Id {
                        editor
                            .draft
                            .value_mut(editor.field)
                            .extend(line.chars().filter(|c| !c.is_control()));
                        editor.error = None;
                    }
                }
            }
            _ if !modifiers.control && !modifiers.alt => {
                if let Some(text) = ev.keystroke.key_char.as_deref() {
                    if !text.chars().any(char::is_control)
                        && (editor.original_id.is_none() || editor.field != AccountField::Id)
                    {
                        editor.draft.value_mut(editor.field).push_str(text);
                        editor.error = None;
                    }
                }
            }
            _ => {}
        }
        cx.notify();
        true
    }

    fn submit_account_editor(&mut self, cx: &mut Context<'_, Self>) {
        let Some(editor) = &self.account_editor else {
            return;
        };
        let mut context = editor.draft.clone().into_config();
        if let Some(original) = &editor.original_id {
            context.id.clone_from(original);
        }
        if let Some(error) = account_validation_error(&context) {
            if let Some(editor) = &mut self.account_editor {
                editor.error = Some(error.into());
            }
            cx.notify();
            return;
        }
        let mut candidate = self.appearance.config.clone();
        if let Some(original) = &editor.original_id {
            if let Some(existing) = candidate
                .contexts
                .iter_mut()
                .find(|existing| existing.id == *original)
            {
                *existing = context.clone();
            }
        } else {
            candidate.contexts.push(context.clone());
        }
        if let Err(error) = candidate.validate() {
            if let Some(editor) = &mut self.account_editor {
                editor.error = Some(format!("Check the account details: {error}"));
            }
            cx.notify();
            return;
        }
        let select_new = self
            .account_editor
            .as_ref()
            .is_some_and(|e| e.original_id.is_none());
        self.appearance.config = candidate;
        self.appearance.save();
        self.account_editor = None;
        if select_new && self.local_project_root.is_some() {
            self.set_context(&context.id, cx);
        } else {
            self.reconcile_context();
            cx.notify();
        }
    }

    fn account_field(
        &self,
        field: AccountField,
        label: &'static str,
        placeholder: &'static str,
        editable: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let editor = self.account_editor.as_ref().expect("editor is open");
        let t = &self.theme;
        let value = editor.draft.value(field);
        let focused = editor.field == field;
        let empty = value.is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(
                div()
                    .text_size(px(tokens::text::XS))
                    .text_color(hsla(t.text_faint))
                    .child(label),
            )
            .child(
                div()
                    .id(SharedString::from(format!("account-field-{label}")))
                    .h(px(34.0))
                    .px(px(tokens::space::SM))
                    .flex()
                    .items_center()
                    .rounded(px(tokens::radius::MD))
                    .bg(hsla(if editable { t.bg_app } else { t.bg_panel }))
                    .border_1()
                    .border_color(hsla(if focused {
                        t.focus_ring
                    } else {
                        t.border_strong
                    }))
                    .text_color(hsla(if empty { t.text_faint } else { t.text }))
                    .when(editable, |d| {
                        d.cursor_text().on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = &mut this.account_editor {
                                editor.field = field;
                            }
                            cx.notify();
                        }))
                    })
                    .child(SharedString::from(if empty {
                        placeholder.to_owned()
                    } else {
                        format!("{value}{}", if focused && editable { "│" } else { "" })
                    })),
            )
    }

    pub(super) fn account_editor_overlay(
        &self,
        cx: &mut Context<'_, Self>,
    ) -> Option<impl IntoElement> {
        let editor = self.account_editor.as_ref()?;
        let t = &self.theme;
        let editing = editor.original_id.is_some();
        Some(
            div()
                .id("account-editor-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(with_alpha(hsla(t.bg_app), 0.82))
                .child(
                    div()
                        .id("account-editor")
                        .w(px(620.0))
                        .max_h(px(680.0))
                        .p(px(tokens::space::XL))
                        .flex()
                        .flex_col()
                        .gap(px(tokens::space::MD))
                        .rounded(px(tokens::radius::XL))
                        .bg(hsla(t.bg_elevated))
                        .border_1()
                        .border_color(hsla(t.border_strong))
                        .child(
                            div()
                                .text_size(px(tokens::text::LG))
                                .text_color(hsla(t.text))
                                .child(if editing { "Edit account" } else { "Add account" }),
                        )
                        .child(
                            div()
                                .text_size(px(tokens::text::SM))
                                .text_color(hsla(t.text_muted))
                                .child("Set the identity applied to new or restarted terminals. Existing terminals are unchanged."),
                        )
                        .child(
                            div()
                                .grid()
                                .grid_cols(2)
                                .gap(px(tokens::space::MD))
                                .child(self.account_field(AccountField::Name, "NAME", "Client A", true, cx))
                                .child(self.account_field(AccountField::Id, "ID", "client-a", !editing, cx))
                                .child(self.account_field(AccountField::GitName, "GIT AUTHOR NAME", "Your Name", true, cx))
                                .child(self.account_field(AccountField::GitEmail, "GIT AUTHOR EMAIL", "you@client.example", true, cx))
                                .child(self.account_field(AccountField::Color, "BADGE COLOUR", "#168aad", true, cx)),
                        )
                        .child({
                            let expanded = editor.show_advanced;
                            div()
                                .id("account-advanced-toggle")
                                .flex()
                                .items_center()
                                .justify_between()
                                .cursor_pointer()
                                .py(px(tokens::space::SM))
                                .border_t_1()
                                .border_color(hsla(t.border))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(editor) = &mut this.account_editor {
                                        editor.show_advanced = !editor.show_advanced;
                                        if !editor.show_advanced && editor.field.is_advanced() {
                                            editor.field = AccountField::Name;
                                        }
                                    }
                                    cx.notify();
                                }))
                                .child(
                                    div()
                                        .text_size(px(tokens::text::SM))
                                        .text_color(hsla(t.text))
                                        .child("Advanced connection settings"),
                                )
                                .child(
                                    div()
                                        .text_size(px(tokens::text::XS))
                                        .text_color(hsla(t.text_faint))
                                        .child(if expanded { "Hide" } else { "Optional" }),
                                )
                        })
                        .when(editor.show_advanced, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(tokens::space::MD))
                                    .child(
                                        div()
                                            .text_size(px(tokens::text::XS))
                                            .text_color(hsla(t.text_faint))
                                            .child("Only set these when an account uses a separate GitHub CLI or Git configuration."),
                                    )
                                    .child(
                                        div()
                                            .grid()
                                            .grid_cols(2)
                                            .gap(px(tokens::space::MD))
                                            .child(self.account_field(AccountField::GithubHost, "EXPECTED REMOTE HOST", "github.com", true, cx))
                                            .child(self.account_field(AccountField::GhConfigDir, "GH CONFIG DIRECTORY", "/home/you/.config/gh-client", true, cx))
                                            .child(self.account_field(AccountField::GitConfigGlobal, "GLOBAL GIT CONFIG", "/home/you/.config/git/client", true, cx)),
                                    ),
                            )
                        })
                        .children(editor.error.as_ref().map(|error| {
                            div()
                                .text_size(px(tokens::text::SM))
                                .text_color(hsla(t.danger))
                                .child(SharedString::from(error.clone()))
                        }))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(tokens::space::SM))
                                .child(self.button(
                                    "account-editor-cancel",
                                    "Cancel",
                                    cx.listener(|this, _, _, cx| {
                                        this.account_editor = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(self.button(
                                    "account-editor-save",
                                    "Save",
                                    cx.listener(|this, _, _, cx| this.submit_account_editor(cx)),
                                )),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_paths_are_none_and_values_are_trimmed() {
        let draft = AccountDraft {
            id: " client-a ".into(),
            name: " Client A ".into(),
            color: " #168aad ".into(),
            git_name: String::new(),
            git_email: String::new(),
            github_host: " github.com ".into(),
            gh_config_dir: "  ".into(),
            git_config_global: " /config/git ".into(),
        };
        let context = draft.into_config();
        assert_eq!(context.id, "client-a");
        assert_eq!(context.name, "Client A");
        assert_eq!(context.gh_config_dir, None);
        assert_eq!(
            context.git_config_global.as_deref(),
            Some(std::path::Path::new("/config/git"))
        );
    }

    #[test]
    fn account_validation_explains_identity_requirements() {
        let missing_id = ContextConfig {
            id: String::new(),
            name: "Client A".into(),
            ..ContextConfig::default()
        };
        assert_eq!(
            account_validation_error(&missing_id),
            Some("Give this account a short ID, such as `client-a`.")
        );

        let invalid_id = ContextConfig {
            id: "Client A".into(),
            name: "Client A".into(),
            ..ContextConfig::default()
        };
        assert_eq!(
            account_validation_error(&invalid_id),
            Some("Account IDs use lowercase letters, numbers, and hyphens only.")
        );
    }

    #[test]
    fn advanced_fields_are_connection_configuration() {
        assert!(AccountField::GithubHost.is_advanced());
        assert!(AccountField::GhConfigDir.is_advanced());
        assert!(AccountField::GitConfigGlobal.is_advanced());
        assert!(!AccountField::GitEmail.is_advanced());
    }
}
