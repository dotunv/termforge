//! In-terminal search state and the search bar.

use super::*;

impl TerminalView {
    pub(super) fn open_search(&mut self, cx: &mut Context<'_, Self>) {
        let prefill = self
            .selection_text()
            .filter(|t| !t.contains('\n') && t.chars().count() <= 200);
        let search = self.search.get_or_insert_with(SearchState::default);
        if let Some(text) = prefill {
            search.query = text;
            self.selection = None;
        }
        self.run_search(cx);
    }

    pub(super) fn close_search(&mut self, cx: &mut Context<'_, Self>) {
        self.search = None;
        cx.notify();
    }

    /// Recompute matches and jump to the one nearest the bottom of the
    /// viewport, searching upwards (terminal output reads bottom-up).
    pub(super) fn run_search(&mut self, cx: &mut Context<'_, Self>) {
        let Some(session) = &self.session else { return };
        let Some(search) = &mut self.search else {
            return;
        };
        let (matches, bottom) = session.with(|p| {
            let e = p.engine();
            let m = find_all(e, &search.query, search.case_sensitive, MAX_MATCHES);
            (
                m,
                e.history_size() - e.display_offset() + e.size().rows as usize,
            )
        });
        search.current = matches
            .iter()
            .rposition(|m| m.start.line < bottom)
            .or_else(|| matches.len().checked_sub(1));
        search.matches = matches;
        search.stale = false;
        self.reveal_current(cx);
    }

    pub(super) fn step_search(&mut self, up: bool, cx: &mut Context<'_, Self>) {
        if self.search.as_ref().is_some_and(|s| s.stale) {
            // Keep the position: re-find, then continue from the old match.
            let old = self
                .search
                .as_ref()
                .and_then(|s| s.current.and_then(|i| s.matches.get(i)).copied());
            self.run_search(cx);
            if let (Some(old), Some(s)) = (old, &mut self.search) {
                if let Some(i) = s.matches.iter().position(|m| *m == old) {
                    s.current = Some(i);
                }
            }
        }
        let Some(s) = &mut self.search else { return };
        let n = s.matches.len();
        if n == 0 {
            return;
        }
        s.current = Some(match (s.current, up) {
            (Some(i), true) => (i + n - 1) % n,
            (Some(i), false) => (i + 1) % n,
            (None, _) => n - 1,
        });
        self.reveal_current(cx);
    }

    /// Scroll so the current match is visible, centred when it was not.
    pub(super) fn reveal_current(&mut self, cx: &mut Context<'_, Self>) {
        let Some(line) = self
            .search
            .as_ref()
            .and_then(|s| s.current.and_then(|i| s.matches.get(i)))
            .map(|m| m.start.line)
        else {
            cx.notify();
            return;
        };
        if let Some(session) = &self.session {
            session.with_mut(|p| {
                let e = p.engine_mut();
                let history = e.history_size();
                let rows = e.size().rows as usize;
                let offset = e.display_offset();
                let top = history - offset;
                if line < top || line >= top + rows {
                    let want_top = line.saturating_sub(rows / 2).min(history);
                    let want_offset = history - want_top;
                    e.scroll(Scroll::Lines(want_offset as i32 - offset as i32));
                }
            });
        }
        cx.notify();
    }

    /// Keys while the search bar is open. Returns whether it was consumed.
    pub(super) fn search_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<'_, Self>) -> bool {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let key = ks.key.as_str();
        let Some(search) = &mut self.search else {
            return false;
        };
        match key {
            "escape" => self.close_search(cx),
            "enter" => self.step_search(!m.shift, cx),
            "up" => self.step_search(true, cx),
            "down" => self.step_search(false, cx),
            "backspace" => {
                if m.control {
                    search.query.clear();
                } else {
                    search.query.pop();
                }
                self.run_search(cx);
            }
            "c" if m.alt && !m.control => {
                search.case_sensitive = !search.case_sensitive;
                self.run_search(cx);
            }
            "f" if m.control && m.shift => {}
            "v" if m.control => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    search
                        .query
                        .push_str(text.lines().next().unwrap_or_default());
                    self.run_search(cx);
                }
            }
            _ if !m.control && !m.alt => match ks.key_char.as_deref() {
                Some(ch) if !ch.chars().any(char::is_control) => {
                    search.query.push_str(ch);
                    self.run_search(cx);
                }
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    pub(super) fn search_bar(&self) -> Option<impl IntoElement> {
        let s = self.search.as_ref()?;
        let t = &self.theme;
        let count = match (s.current, s.matches.len()) {
            (_, 0) if s.query.is_empty() => String::new(),
            (_, 0) => "No results".to_owned(),
            (Some(i), n) if n >= MAX_MATCHES => format!("{} of {n}+", i + 1),
            (Some(i), n) => format!("{} of {n}", i + 1),
            (None, n) => format!("{n}"),
        };
        let pill = |label: &'static str, on: bool| {
            div()
                .px(px(tokens::space::XS + 2.0))
                .rounded(px(tokens::radius::SM))
                .text_size(px(tokens::text::XS))
                .when(on, |d| d.bg(hsla(t.accent)).text_color(hsla(t.accent_text)))
                .when(!on, |d| d.text_color(hsla(t.text_faint)))
                .child(label)
        };
        Some(
            div()
                .absolute()
                .top(px(72.0 + tokens::space::SM))
                .right(px(tokens::space::LG))
                .h(px(30.0))
                .min_w(px(300.0))
                .px(px(tokens::space::MD))
                .flex()
                .items_center()
                .gap(px(tokens::space::SM))
                .rounded(px(tokens::radius::LG))
                .bg(hsla(t.bg_elevated))
                .border_1()
                .border_color(hsla(t.border_strong))
                .shadow_md()
                .child(
                    div()
                        .text_color(hsla(t.text_faint))
                        .text_size(px(tokens::text::XS))
                        .child("Find"),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .text_color(hsla(t.text))
                        .child(SharedString::from(s.query.clone()))
                        .child(div().w(px(1.5)).h(px(15.0)).bg(hsla(t.accent))),
                )
                .child(
                    div()
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(if count == "No results" {
                            t.danger
                        } else {
                            t.text_muted
                        }))
                        .child(SharedString::from(count)),
                )
                .child(pill("Aa", s.case_sensitive))
                .child(
                    div()
                        .text_size(px(tokens::text::XS))
                        .text_color(hsla(t.text_faint))
                        .child("↑↓  Esc"),
                ),
        )
    }
}
