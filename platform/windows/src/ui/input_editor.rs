//! Anchored input editor — Warp-style app-owned command input.
//!
//! Instead of forwarding every keystroke to the PTY, the active pane gets a
//! persistent input bar anchored above the status bar.  The app owns the
//! text buffer, so cursor movement, editing, and history are real editor
//! operations rather than escape-sequence round-trips.
//!
//! Safety rules (v1):
//! - The editor only arms when shell integration (OSC 133 blocks) shows the
//!   shell is *idle at a prompt*: the session has at least one block and the
//!   last block is finished.  While a command runs — vim, ssh, anything —
//!   keys pass through raw, exactly like a classic terminal.
//! - Shells without integration never produce blocks, so they keep classic
//!   behaviour entirely.
//! - Tab hands the half-typed line to the shell (without Enter) and drops to
//!   passthrough so native completion keeps working; the editor re-arms at
//!   the next prompt after a command runs.

use renderer_windows::tokens::*;
use renderer_windows::ui_renderer::UiCommand;

const HISTORY_MAX: usize = 100;

/// What the app should do with the PTY in response to an editor keystroke.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorEffect {
    /// Nothing to send; the editor consumed the key.
    None,
    /// Send these bytes to the PTY (submit, interrupt, or tab-flush).
    Send(Vec<u8>),
}

/// Editor interaction mode.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorMode {
    Edit,
    /// Ctrl+R reverse history search: query plus the index of the current
    /// match (0 = most recent).
    Search { query: String, nth: usize },
}

pub struct InputEditor {
    pub buffer: String,
    /// Cursor position in chars (0..=buffer chars).
    pub cursor: usize,
    pub mode: EditorMode,
    history: Vec<String>,
    /// Index into history while browsing with ↑/↓; None = editing a new line.
    history_idx: Option<usize>,
    /// Holds the buffer that was being typed before history browsing began.
    stash: String,
    /// Set after a Tab flush: block count at flush time. The editor stays in
    /// passthrough until a *new* block has started and finished (i.e. the
    /// shell ran the completed command and returned to a prompt).
    pub passthrough_since: Option<usize>,
}

impl InputEditor {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            mode: EditorMode::Edit,
            history: Vec::new(),
            history_idx: None,
            stash: String::new(),
            passthrough_since: None,
        }
    }

    fn char_len(&self) -> usize {
        self.buffer.chars().count()
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.buffer.len())
    }

    pub fn insert(&mut self, c: char) {
        let b = self.byte_at(self.cursor);
        self.buffer.insert(b, c);
        self.cursor += 1;
        self.history_idx = None;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let b = self.byte_at(self.cursor - 1);
            self.buffer.remove(b);
            self.cursor -= 1;
        }
        self.history_idx = None;
    }

    pub fn delete(&mut self) {
        if self.cursor < self.char_len() {
            let b = self.byte_at(self.cursor);
            self.buffer.remove(b);
        }
        self.history_idx = None;
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// → at the end of the buffer accepts the ghost suggestion (fish-style);
    /// anywhere else it just moves the cursor.
    pub fn move_right(&mut self) {
        if self.cursor == self.char_len() {
            self.accept_ghost();
        } else {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// End also accepts the ghost suggestion when already at the end.
    pub fn end(&mut self) {
        if self.cursor == self.char_len() {
            self.accept_ghost();
        }
        self.cursor = self.char_len();
    }

    // ── Ghost autosuggestion (most recent history entry with this prefix) ──

    /// The faint completion shown after the caret, if any.
    pub fn ghost_suffix(&self) -> Option<String> {
        if self.mode != EditorMode::Edit
            || self.buffer.is_empty()
            || self.cursor != self.char_len()
        {
            return None;
        }
        self.history
            .iter()
            .rev()
            .find(|h| h.starts_with(&self.buffer) && h.len() > self.buffer.len())
            .map(|h| h[self.buffer.len()..].to_string())
    }

    fn accept_ghost(&mut self) {
        if let Some(suffix) = self.ghost_suffix() {
            self.buffer.push_str(&suffix);
            self.cursor = self.char_len();
        }
    }

    // ── Ctrl+R reverse history search ───────────────────────────────────────

    pub fn start_search(&mut self) {
        match &mut self.mode {
            // Ctrl+R while already searching steps to the next-older match.
            EditorMode::Search { nth, .. } => *nth += 1,
            EditorMode::Edit => self.mode = EditorMode::Search { query: String::new(), nth: 0 },
        }
    }

    pub fn search_push(&mut self, c: char) {
        if let EditorMode::Search { query, nth } = &mut self.mode {
            query.push(c);
            *nth = 0;
        }
    }

    pub fn search_backspace(&mut self) {
        if let EditorMode::Search { query, nth } = &mut self.mode {
            query.pop();
            *nth = 0;
        }
    }

    /// The history entry the current search selects, if any.
    pub fn search_match(&self) -> Option<&str> {
        let EditorMode::Search { query, nth } = &self.mode else { return None };
        self.history
            .iter()
            .rev()
            .filter(|h| h.contains(query.as_str()))
            .nth(*nth)
            .map(String::as_str)
    }

    /// Leave search mode. `accept` pulls the match into the buffer.
    pub fn end_search(&mut self, accept: bool) {
        if accept {
            if let Some(m) = self.search_match().map(str::to_string) {
                self.buffer = m;
                self.cursor = self.char_len();
            }
        }
        self.mode = EditorMode::Edit;
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_idx {
            None => {
                self.stash = self.buffer.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(next);
        self.buffer = self.history[next].clone();
        self.cursor = self.char_len();
    }

    pub fn history_next(&mut self) {
        match self.history_idx {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.history_idx = Some(i + 1);
                self.buffer = self.history[i + 1].clone();
                self.cursor = self.char_len();
            }
            Some(_) => {
                self.history_idx = None;
                self.buffer = self.stash.clone();
                self.cursor = self.char_len();
            }
        }
    }

    /// Enter: submit the line. Returns the bytes to write to the PTY.
    pub fn submit(&mut self) -> EditorEffect {
        let line = self.buffer.clone();
        if !line.trim().is_empty()
            && self.history.last().map(String::as_str) != Some(line.as_str())
        {
            self.history.push(line.clone());
            if self.history.len() > HISTORY_MAX {
                self.history.remove(0);
            }
        }
        self.buffer.clear();
        self.cursor = 0;
        self.history_idx = None;
        let mut bytes = line.into_bytes();
        bytes.push(b'\r');
        EditorEffect::Send(bytes)
    }

    /// Tab: flush the partial line to the shell (no Enter) so native
    /// completion takes over; the caller switches to passthrough.
    pub fn flush_for_completion(&mut self, current_block_count: usize) -> EditorEffect {
        let mut bytes = std::mem::take(&mut self.buffer).into_bytes();
        bytes.push(b'\t');
        self.cursor = 0;
        self.history_idx = None;
        self.passthrough_since = Some(current_block_count);
        EditorEffect::Send(bytes)
    }

    /// Ctrl+C at the prompt: drop the draft and forward the interrupt.
    pub fn interrupt(&mut self) -> EditorEffect {
        self.buffer.clear();
        self.cursor = 0;
        self.history_idx = None;
        EditorEffect::Send(vec![0x03])
    }

    /// Re-arm after a Tab flush once the shell has run a command and returned
    /// to a prompt (block count grew past the flush point).
    pub fn maybe_rearm(&mut self, current_block_count: usize) {
        if let Some(since) = self.passthrough_since {
            if current_block_count > since {
                self.passthrough_since = None;
            }
        }
    }
}

/// Basic shell-aware highlighting: first word = command (cyan), `-flags`
/// (blue), quoted strings (amber), everything else default text.
pub fn highlight_spans(line: &str) -> Vec<(String, [f32; 4])> {
    let mut spans: Vec<(String, [f32; 4])> = Vec::new();
    let mut cur = String::new();
    let mut cur_col = TEXT_PRIMARY;
    let mut seen_cmd = false;
    let mut in_quote: Option<char> = None;

    let flush = |spans: &mut Vec<(String, [f32; 4])>, cur: &mut String, col: [f32; 4]| {
        if !cur.is_empty() {
            spans.push((std::mem::take(cur), col));
        }
    };

    let chars = line.chars().peekable();
    for c in chars {
        if let Some(q) = in_quote {
            cur.push(c);
            if c == q {
                flush(&mut spans, &mut cur, AMBER);
                in_quote = None;
                cur_col = TEXT_PRIMARY;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                flush(&mut spans, &mut cur, cur_col);
                in_quote = Some(c);
                cur.push(c);
                cur_col = AMBER;
            }
            ' ' => {
                flush(&mut spans, &mut cur, cur_col);
                if !spans.is_empty() {
                    seen_cmd = true;
                }
                cur.push(' ');
                flush(&mut spans, &mut cur, TEXT_PRIMARY);
                cur_col = TEXT_PRIMARY;
            }
            '-' if cur.is_empty() && seen_cmd => {
                cur.push(c);
                cur_col = BLUE;
            }
            _ => {
                if cur.is_empty() {
                    cur_col = if seen_cmd { TEXT_PRIMARY } else { CYAN };
                }
                cur.push(c);
            }
        }
    }
    flush(&mut spans, &mut cur, if in_quote.is_some() { AMBER } else { cur_col });
    spans
}

/// Render the input bar across the bottom of the active pane.
/// `accent` is the session-type colour (mint/cornflower/lilac).
pub fn generate_input_bar(
    editor: &InputEditor,
    x: f32,
    y: f32,
    w: f32,
    cell_w: f32,
    cell_h: f32,
    accent: [f32; 4],
) -> Vec<UiCommand> {
    let bar_h = bar_height(cell_h);
    let mut cmds = Vec::with_capacity(8);

    cmds.push(UiCommand::FillRect {
        x, y, w, h: bar_h,
        color: BG_SURFACE,
    });
    cmds.push(UiCommand::FillRect {
        x, y, w, h: 1.0,
        color: BORDER_DEFAULT,
    });
    // Accent tick at the left edge — mirrors the active-pane indicator.
    cmds.push(UiCommand::FillRect {
        x, y: y + 4.0, w: 3.0, h: bar_h - 8.0,
        color: accent,
    });

    let text_y = y + (bar_h - cell_h) * 0.5;
    cmds.push(UiCommand::DrawText {
        x: x + 12.0,
        y: text_y,
        text: "\u{276F}".to_string(),
        fg: accent,
        bg: BG_SURFACE,
    });

    let text_x = x + 12.0 + cell_w * 2.0;

    // Ctrl+R reverse search renders its own bar content.
    if let EditorMode::Search { query, .. } = &editor.mode {
        let m = editor.search_match().unwrap_or("");
        cmds.push(UiCommand::DrawText {
            x: text_x,
            y: text_y,
            text: format!("(reverse-i-search) {query}_ : {m}"),
            fg: if m.is_empty() && !query.is_empty() { TEXT_FAINT } else { TEXT_PRIMARY },
            bg: BG_SURFACE,
        });
        return cmds;
    }

    if editor.buffer.is_empty() {
        cmds.push(UiCommand::DrawUiText {
            x: text_x,
            y: text_y,
            text: "Type a command  \u{00B7}  Enter run  \u{00B7}  Tab complete  \u{00B7}  \u{2191} history  \u{00B7}  Ctrl+R search".to_string(),
            fg: TEXT_FAINT,
            bg: BG_SURFACE,
        });
    } else {
        // Highlighted spans (command / flags / strings), monospace so the
        // caret position math stays char-aligned.
        let mut sx = text_x;
        for (span, col) in highlight_spans(&editor.buffer) {
            let w = span.chars().count() as f32 * cell_w;
            cmds.push(UiCommand::DrawText {
                x: sx,
                y: text_y,
                text: span,
                fg: col,
                bg: BG_SURFACE,
            });
            sx += w;
        }
        // Ghost autosuggestion after the buffer (accept with → or End).
        if let Some(ghost) = editor.ghost_suffix() {
            cmds.push(UiCommand::DrawText {
                x: sx,
                y: text_y,
                text: ghost,
                fg: TEXT_FAINT,
                bg: BG_SURFACE,
            });
        }
    }

    // Caret: a 2px bar at the cursor's character position.
    let caret_x = text_x + editor.cursor as f32 * cell_w;
    cmds.push(UiCommand::FillRect {
        x: caret_x,
        y: text_y,
        w: 2.0,
        h: cell_h,
        color: accent,
    });

    cmds
}

pub fn bar_height(cell_h: f32) -> f32 {
    cell_h + 14.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_and_cursor() {
        let mut e = InputEditor::new();
        for c in "cargo".chars() {
            e.insert(c);
        }
        e.move_left();
        e.move_left();
        e.insert('X');
        assert_eq!(e.buffer, "carXgo");
        e.backspace();
        assert_eq!(e.buffer, "cargo");
        e.end();
        e.delete(); // no-op at end
        assert_eq!(e.buffer, "cargo");
    }

    #[test]
    fn submit_appends_cr_and_records_history() {
        let mut e = InputEditor::new();
        for c in "ls".chars() {
            e.insert(c);
        }
        assert_eq!(e.submit(), EditorEffect::Send(b"ls\r".to_vec()));
        assert!(e.buffer.is_empty());
        e.history_prev();
        assert_eq!(e.buffer, "ls");
    }

    #[test]
    fn history_browsing_preserves_draft() {
        let mut e = InputEditor::new();
        for cmd in ["first", "second"] {
            for c in cmd.chars() {
                e.insert(c);
            }
            e.submit();
        }
        for c in "dra".chars() {
            e.insert(c);
        }
        e.history_prev();
        assert_eq!(e.buffer, "second");
        e.history_prev();
        assert_eq!(e.buffer, "first");
        e.history_next();
        assert_eq!(e.buffer, "second");
        e.history_next();
        assert_eq!(e.buffer, "dra"); // draft restored
    }

    #[test]
    fn ghost_suggests_recent_history_prefix() {
        let mut e = InputEditor::new();
        for cmd in ["cargo test", "cargo build"] {
            for c in cmd.chars() {
                e.insert(c);
            }
            e.submit();
        }
        for c in "cargo".chars() {
            e.insert(c);
        }
        assert_eq!(e.ghost_suffix(), Some(" build".to_string())); // most recent wins
        e.move_right(); // accept at end
        assert_eq!(e.buffer, "cargo build");
        assert_eq!(e.ghost_suffix(), None);
    }

    #[test]
    fn reverse_search_finds_and_cycles() {
        let mut e = InputEditor::new();
        for cmd in ["git status", "cargo test", "git push"] {
            for c in cmd.chars() {
                e.insert(c);
            }
            e.submit();
        }
        e.start_search();
        for c in "git".chars() {
            e.search_push(c);
        }
        assert_eq!(e.search_match(), Some("git push"));
        e.start_search(); // Ctrl+R again → older match
        assert_eq!(e.search_match(), Some("git status"));
        e.end_search(true);
        assert_eq!(e.buffer, "git status");
        assert_eq!(e.mode, EditorMode::Edit);
    }

    #[test]
    fn highlight_command_flags_strings() {
        let spans = highlight_spans("git commit -m \"fix bug\"");
        let find = |text: &str| spans.iter().find(|(s, _)| s == text).map(|(_, c)| *c);
        assert_eq!(find("git"), Some(CYAN));
        assert_eq!(find("commit"), Some(TEXT_PRIMARY));
        assert_eq!(find("-m"), Some(BLUE));
        assert_eq!(find("\"fix bug\""), Some(AMBER));
    }

    #[test]
    fn tab_flush_enters_passthrough_and_rearms() {
        let mut e = InputEditor::new();
        for c in "car".chars() {
            e.insert(c);
        }
        assert_eq!(e.flush_for_completion(3), EditorEffect::Send(b"car\t".to_vec()));
        assert_eq!(e.passthrough_since, Some(3));
        e.maybe_rearm(3); // no new block yet
        assert_eq!(e.passthrough_since, Some(3));
        e.maybe_rearm(4); // a command ran → prompt is back
        assert_eq!(e.passthrough_since, None);
    }
}
