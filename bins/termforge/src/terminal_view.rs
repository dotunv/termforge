//! The terminal view: owns one session and turns GPUI input into terminal
//! input, selections and searches.
//!
//! Everything terminal-specific (emulation, key and mouse encoding, text
//! selection, search, block tracking, colours) lives in framework-free
//! crates; this file only wires those onto GPUI events. Painting is in
//! `paint.rs`.

use std::sync::Arc;

use futures::StreamExt;
use gpui::{
    canvas, div, prelude::*, px, App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase,
    Entity, FocusHandle, Hitbox, HitboxBehavior, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollWheelEvent, SharedString,
    Subscription, Task, Window,
};
use tf_engine::text::{find_all, is_openable, link_at, Link, Match};
use tf_engine::{
    GridSize, MouseTracking, Point, Scroll, Selection, SelectionKind, Snapshot, TerminalEngine,
};
use tf_input::{
    encode_focus, encode_key, encode_mouse, encode_paste, InputModes, Key, Mods, MouseAction,
    MouseButton as TMouse, Tracking,
};
use tf_session::{LiveSession, SessionEvent, SpawnOptions};
use tf_ui::{tokens, Rgb, Theme};

use crate::fonts::MonoFont;
use crate::paint::{hsla, paint_grid, BlockMark, Metrics, Overlay, PaintArgs};

/// Search results beyond this are not collected, so a one-letter query in
/// a full scrollback cannot stall the UI.
const MAX_MATCHES: usize = 5_000;

#[derive(Debug, Default)]
struct SearchState {
    query: String,
    case_sensitive: bool,
    matches: Vec<Match>,
    current: Option<usize>,
    /// New output arrived since `matches` was computed.
    stale: bool,
}

/// A mouse press forwarded to the program (mouse tracking on).
#[derive(Debug, Clone, Copy)]
struct Reported {
    button: TMouse,
    last: (u16, u16),
}

pub struct TerminalView {
    session: Option<Arc<LiveSession>>,
    spawn: SpawnOptions,
    focus: FocusHandle,
    theme: Arc<Theme>,
    font: MonoFont,
    font_size: f32,
    scroll_accum: f32,
    cwd: Option<String>,
    title: Option<String>,
    last_exit: Option<i32>,
    commands: usize,
    error: Option<String>,

    selection: Option<Selection>,
    selecting: bool,
    reported: Option<Reported>,
    last_motion: Option<(u16, u16)>,
    hover_link: Option<Link>,
    search: Option<SearchState>,
    /// Scrollback length at the last output, to notice clears.
    history: usize,
    /// Canvas bounds from the last paint, for mouse hit-testing.
    grid_bounds: Bounds<Pixels>,

    _pump: Option<Task<()>>,
    _focus_subs: Vec<Subscription>,
}

impl TerminalView {
    pub fn new(
        spawn: SpawnOptions,
        theme: Arc<Theme>,
        font: MonoFont,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let subs = vec![
            cx.on_focus_in(&focus, window, |this, _, cx| this.focus_changed(true, cx)),
            cx.on_focus_out(&focus, window, |this, _, _, cx| {
                this.focus_changed(false, cx);
            }),
        ];
        let mut view = Self {
            session: None,
            spawn,
            focus,
            theme,
            font,
            font_size: 14.0,
            scroll_accum: 0.0,
            cwd: None,
            title: None,
            last_exit: None,
            commands: 0,
            error: None,
            selection: None,
            selecting: false,
            reported: None,
            last_motion: None,
            hover_link: None,
            search: None,
            history: 0,
            grid_bounds: Bounds::default(),
            _pump: None,
            _focus_subs: subs,
        };
        view.start(cx);
        view
    }

    fn start(&mut self, cx: &mut Context<'_, Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let waker: tf_session::Waker = Arc::new(move || {
            let _ = tx.unbounded_send(());
        });
        match LiveSession::spawn(self.spawn.clone(), waker) {
            Ok(session) => {
                self.session = Some(Arc::new(session));
                self.error = None;
                self.last_exit = None;
                self.commands = 0;
            }
            Err(e) => {
                tracing::error!("failed to start shell: {e:#}");
                self.error = Some(format!("{e:#}"));
                self.session = None;
            }
        }
        self.selection = None;
        self.search = None;
        self.history = 0;
        self._pump = Some(cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                // Coalesce bursts: one repaint per batch of wakes.
                while rx.try_recv().is_ok() {}
                if this.update(cx, |view, cx| view.on_output(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn on_output(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(session) = &self.session {
            for event in session.take_events() {
                match event {
                    SessionEvent::Cwd(cwd) => self.cwd = Some(cwd),
                    SessionEvent::BlockFinished { exit_code, .. } => {
                        self.last_exit = exit_code;
                        self.commands += 1;
                    }
                    SessionEvent::Notify { title, body } => {
                        tracing::info!(?title, %body, "terminal notification");
                    }
                }
            }
            let (title, history) =
                session.with(|p| (p.engine().title(), p.engine().history_size()));
            self.title = title;
            // Scrollback shrank (`clear`): absolute positions now point at
            // different content, so drop anything anchored to them.
            if history < self.history {
                self.selection = None;
                if let Some(s) = &mut self.search {
                    s.matches.clear();
                    s.current = None;
                }
            }
            self.history = history;
            if let Some(s) = &mut self.search {
                s.stale = true;
            }
        }
        cx.notify();
    }

    fn metrics(&self, window: &Window) -> Metrics {
        let ts = window.text_system();
        let font_size = px(self.font_size);
        let id = ts.resolve_font(&self.font.regular());
        let cell_w = ts
            .advance(id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(px(self.font_size * 0.6));
        Metrics {
            cell_w,
            line_h: px((self.font_size * 1.35).round()),
        }
    }

    fn write(&self, bytes: &[u8]) {
        if let Some(s) = &self.session {
            if let Err(e) = s.write(bytes) {
                tracing::warn!("write to pty failed: {e:#}");
            }
        }
    }

    fn modes(&self) -> tf_engine::Modes {
        self.session
            .as_ref()
            .map(|s| s.with(|p| p.engine().modes()))
            .unwrap_or_default()
    }

    fn input_modes(&self) -> InputModes {
        let m = self.modes();
        InputModes {
            app_cursor: m.app_cursor,
            bracketed_paste: m.bracketed_paste,
        }
    }

    fn exited(&self) -> Option<Option<u32>> {
        self.session.as_ref().and_then(|s| s.exit_status())
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<'_, Self>) {
        if self.modes().focus_events {
            self.write(encode_focus(focused));
        }
        cx.notify();
    }

    // ---- keyboard -------------------------------------------------------

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let mods = Mods {
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
        };
        let key = ks.key.as_str();
        let ctrl_shift = m.control && m.shift && !m.alt;

        if self.search.is_some() && self.search_key(ev, cx) {
            cx.stop_propagation();
            return;
        }

        // App shortcuts first. Ctrl+Shift chords never collide with shells.
        if ctrl_shift && key.eq_ignore_ascii_case("f") {
            self.open_search(cx);
        } else if (ctrl_shift && key.eq_ignore_ascii_case("v")) || (m.shift && key == "insert") {
            self.paste(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("c") {
            self.copy_selection(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("a") {
            self.select_all(cx);
        } else if m.control && !m.shift && !m.alt && key == "c" && self.has_selection() {
            // Windows Terminal convention: Ctrl+C copies when something is
            // selected, and is ^C otherwise.
            self.copy_selection(cx);
            self.clear_selection(cx);
        } else if key == "escape" && self.has_selection() {
            self.clear_selection(cx);
        } else if m.control && !m.alt && !m.shift && matches!(key, "=" | "+" | "-" | "0") {
            self.font_size = match key {
                "-" => (self.font_size - 1.0).max(8.0),
                "0" => 14.0,
                _ => (self.font_size + 1.0).min(32.0),
            };
            cx.notify();
        } else if m.shift && !m.control && matches!(key, "pageup" | "pagedown") {
            let s = if key == "pageup" {
                Scroll::PageUp
            } else {
                Scroll::PageDown
            };
            self.scroll(s, cx);
        } else if m.shift && m.control && matches!(key, "home" | "end") {
            let s = if key == "home" {
                Scroll::Top
            } else {
                Scroll::Bottom
            };
            self.scroll(s, cx);
        } else {
            self.send_key(ev, mods, window, cx);
            return;
        }
        cx.stop_propagation();
    }

    fn send_key(
        &mut self,
        ev: &KeyDownEvent,
        mods: Mods,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let ks = &ev.keystroke;
        let key = ks.key.as_str();
        if let Some(Some(_)) = self.exited() {
            if key == "enter" {
                self.start(cx);
                window.focus(&self.focus);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        let logical = Key::from_name(key).or_else(|| {
            // Prefer the layout-resolved character; fall back to the key
            // itself for Ctrl chords, where no character is produced.
            ks.key_char
                .clone()
                .or_else(|| (mods.ctrl || mods.alt).then(|| key.to_owned()))
                .map(Key::Text)
        });
        let Some(logical) = logical else { return };
        if let Some(bytes) = encode_key(&logical, mods, self.input_modes()) {
            self.selection = None;
            self.scroll(Scroll::Bottom, cx);
            self.write(&bytes);
            cx.stop_propagation();
        }
    }

    fn paste(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let bytes = encode_paste(&text, self.input_modes());
            self.selection = None;
            self.scroll(Scroll::Bottom, cx);
            self.write(&bytes);
        }
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<'_, Self>) {
        if let Some(s) = &self.session {
            s.with_mut(|p| p.engine_mut().scroll(scroll));
            cx.notify();
        }
    }

    // ---- selection ------------------------------------------------------

    fn has_selection(&self) -> bool {
        self.selection.is_some_and(|s| !s.is_empty())
    }

    fn selection_text(&self) -> Option<String> {
        let sel = self.selection.filter(|s| !s.is_empty())?;
        let text = self.session.as_ref()?.with(|p| sel.text(p.engine()));
        (!text.is_empty()).then_some(text)
    }

    fn copy_selection(&self, cx: &mut Context<'_, Self>) {
        if let Some(text) = self.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn clear_selection(&mut self, cx: &mut Context<'_, Self>) {
        self.selection = None;
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<'_, Self>) {
        let Some(s) = &self.session else { return };
        let (total, cols) = s.with(|p| (p.engine().total_lines(), p.engine().size().cols));
        let mut sel = Selection::new(Point::new(0, 0), SelectionKind::Cells);
        sel.extend(Point::new(total.saturating_sub(1), cols.saturating_sub(1)));
        self.selection = Some(sel);
        cx.notify();
    }

    // ---- mouse ----------------------------------------------------------

    /// Viewport cell and absolute point under a window position.
    fn hit(&self, pos: gpui::Point<Pixels>, window: &Window) -> Option<((u16, u16), Point)> {
        let s = self.session.as_ref()?;
        let (size, top) = s.with(|p| {
            let e = p.engine();
            (e.size(), e.history_size() - e.display_offset())
        });
        let cell = self
            .metrics(window)
            .cell_at(self.grid_bounds, pos, size.cols, size.rows);
        Some((cell, Point::new(top + cell.1 as usize, cell.0)))
    }

    /// Mouse tracking applies unless Shift is held (Shift always selects).
    fn tracking(&self, mods: Modifiers) -> Tracking {
        if mods.shift {
            return Tracking::Off;
        }
        match self.modes().mouse {
            MouseTracking::Off => Tracking::Off,
            MouseTracking::Click => Tracking::Click,
            MouseTracking::Drag => Tracking::Drag,
            MouseTracking::Motion => Tracking::Motion,
        }
    }

    fn report(&mut self, action: MouseAction, cell: (u16, u16), mods: Modifiers) -> bool {
        let tracking = self.tracking(mods);
        let m = Mods {
            ctrl: mods.control,
            alt: mods.alt,
            shift: mods.shift,
        };
        match encode_mouse(action, cell.0, cell.1, m, tracking, self.modes().sgr_mouse) {
            Some(bytes) => {
                self.write(&bytes);
                true
            }
            None => false,
        }
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        window.focus(&self.focus);
        let Some((cell, at)) = self.hit(ev.position, window) else {
            return;
        };
        if ev.button == MouseButton::Left && ev.modifiers.control {
            if let Some(link) = self.link_at(at) {
                if is_openable(&link.url) {
                    cx.open_url(&link.url);
                }
                return;
            }
        }
        let button = match ev.button {
            MouseButton::Left => Some(TMouse::Left),
            MouseButton::Middle => Some(TMouse::Middle),
            MouseButton::Right => Some(TMouse::Right),
            _ => None,
        };
        if let Some(b) = button {
            if self.report(MouseAction::Press(b), cell, ev.modifiers) {
                self.reported = Some(Reported {
                    button: b,
                    last: cell,
                });
                self.selection = None;
                cx.notify();
                return;
            }
        }
        match ev.button {
            MouseButton::Left => {
                let kind = match ev.click_count {
                    0 | 1 => SelectionKind::Cells,
                    2 => SelectionKind::Words,
                    _ => SelectionKind::Lines,
                };
                match &mut self.selection {
                    Some(sel) if ev.modifiers.shift && ev.click_count <= 1 => sel.extend(at),
                    _ => self.selection = Some(Selection::new(at, kind)),
                }
                self.selecting = true;
            }
            // Windows Terminal convention: right-click copies a selection,
            // or pastes when there is none.
            MouseButton::Right => {
                if self.has_selection() {
                    self.copy_selection(cx);
                    self.selection = None;
                } else {
                    self.paste(cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some((cell, at)) = self.hit(ev.position, window) else {
            return;
        };
        self.update_hover(ev.modifiers, Some(at), cx);

        if let Some(r) = self.reported {
            if cell != r.last {
                self.report(MouseAction::Motion(Some(r.button)), cell, ev.modifiers);
                self.reported = Some(Reported { last: cell, ..r });
            }
            return;
        }
        if self.selecting && ev.pressed_button == Some(MouseButton::Left) {
            // Dragging past the edge scrolls, one line per move event.
            let b = self.grid_bounds;
            if ev.position.y < b.origin.y {
                self.scroll(Scroll::Lines(1), cx);
            } else if ev.position.y > b.origin.y + b.size.height {
                self.scroll(Scroll::Lines(-1), cx);
            }
            if let Some((_, at)) = self.hit(ev.position, window) {
                if let Some(sel) = &mut self.selection {
                    sel.extend(at);
                }
            }
            cx.notify();
            return;
        }
        if ev.pressed_button.is_none()
            && self.tracking(ev.modifiers) == Tracking::Motion
            && self.last_motion != Some(cell)
            && self.grid_bounds.contains(&ev.position)
        {
            self.last_motion = Some(cell);
            self.report(MouseAction::Motion(None), cell, ev.modifiers);
        }
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(r) = self.reported.take() {
            let cell = self.hit(ev.position, window).map_or(r.last, |(c, _)| c);
            self.report(MouseAction::Release(r.button), cell, ev.modifiers);
            return;
        }
        if self.selecting {
            self.selecting = false;
            if self.selection.is_some_and(|s| s.is_empty()) {
                self.selection = None;
            }
            cx.notify();
        }
    }

    fn link_at(&self, at: Point) -> Option<Link> {
        self.session.as_ref()?.with(|p| link_at(p.engine(), at))
    }

    /// Ctrl+hover underlines links, as in VS Code and Windows Terminal.
    fn update_hover(&mut self, mods: Modifiers, at: Option<Point>, cx: &mut Context<'_, Self>) {
        let link = if mods.control && !mods.shift {
            at.and_then(|p| self.link_at(p))
                .filter(|l| is_openable(&l.url))
        } else {
            None
        };
        if link != self.hover_link {
            self.hover_link = link;
            cx.notify();
        }
    }

    fn scroll_wheel(
        &mut self,
        ev: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let line_h = self.metrics(window).line_h;
        self.scroll_accum += ev.delta.pixel_delta(line_h).y / line_h;
        let lines = self.scroll_accum.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;
        let steps = lines.unsigned_abs().min(10);

        if self.tracking(ev.modifiers) != Tracking::Off {
            if let Some((cell, _)) = self.hit(ev.position, window) {
                let b = if lines > 0 {
                    TMouse::WheelUp
                } else {
                    TMouse::WheelDown
                };
                for _ in 0..steps {
                    self.report(MouseAction::Press(b), cell, ev.modifiers);
                }
            }
            return;
        }
        let modes = self.modes();
        if modes.alt_screen && modes.alternate_scroll {
            // Alternate scroll mode: full-screen apps get arrow keys.
            let key = if lines > 0 { Key::Up } else { Key::Down };
            if let Some(b) = encode_key(&key, Mods::NONE, self.input_modes()) {
                for _ in 0..steps {
                    self.write(&b);
                }
            }
        } else {
            self.scroll(Scroll::Lines(lines * 3), cx);
        }
    }

    // ---- search ---------------------------------------------------------

    fn open_search(&mut self, cx: &mut Context<'_, Self>) {
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

    fn close_search(&mut self, cx: &mut Context<'_, Self>) {
        self.search = None;
        cx.notify();
    }

    /// Recompute matches and jump to the one nearest the bottom of the
    /// viewport, searching upwards (terminal output reads bottom-up).
    fn run_search(&mut self, cx: &mut Context<'_, Self>) {
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

    fn step_search(&mut self, up: bool, cx: &mut Context<'_, Self>) {
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
    fn reveal_current(&mut self, cx: &mut Context<'_, Self>) {
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
    fn search_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<'_, Self>) -> bool {
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

    fn search_bar(&self) -> Option<impl IntoElement> {
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
                .top(px(tokens::space::SM))
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

    // ---- chrome ---------------------------------------------------------

    fn header(&self) -> impl IntoElement {
        let t = &self.theme;
        let shell = self.spawn.profile.name.clone();
        let location = self
            .title
            .clone()
            .or_else(|| self.cwd.clone())
            .unwrap_or_default();
        div()
            .h(px(tokens::layout::TAB_H + 6.0))
            .px(px(tokens::space::MD))
            .flex()
            .items_center()
            .gap(px(tokens::space::SM))
            .bg(hsla(t.bg_app))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::space::SM))
                    .h(px(tokens::layout::TAB_H - 4.0))
                    .px(px(tokens::space::MD))
                    .rounded(px(tokens::radius::MD))
                    .bg(hsla(t.bg_elevated))
                    .border_1()
                    .border_color(hsla(t.border))
                    .child(
                        div()
                            .size(px(6.0))
                            .rounded_full()
                            .bg(hsla(self.status_color())),
                    )
                    .child(
                        div()
                            .text_color(hsla(t.text))
                            .child(SharedString::from(shell)),
                    )
                    .child(
                        div()
                            .text_color(hsla(t.text_faint))
                            .max_w(px(420.0))
                            .overflow_hidden()
                            .child(SharedString::from(location)),
                    ),
            )
    }

    fn status_color(&self) -> Rgb {
        let t = &self.theme;
        match (self.exited(), self.last_exit) {
            (Some(_), _) => t.text_faint,
            (None, Some(c)) if c != 0 => t.danger,
            _ => t.success,
        }
    }

    fn status_bar(&self, grid: Option<GridSize>) -> impl IntoElement {
        let t = &self.theme;
        let mut left = format!(
            "{} command{}",
            self.commands,
            if self.commands == 1 { "" } else { "s" }
        );
        if let Some(code) = self.last_exit {
            left.push_str(&format!("  ·  last exit {code}"));
        }
        let mut right = String::new();
        if self.hover_link.is_some() {
            right.push_str("Ctrl+click to open  ·  ");
        }
        if let Some(g) = grid {
            right.push_str(&format!("{}×{}", g.cols, g.rows));
        }
        div()
            .h(px(tokens::layout::STATUSBAR_H))
            .px(px(tokens::space::MD))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(tokens::text::XS))
            .text_color(hsla(t.text_faint))
            .bg(hsla(t.bg_app))
            .border_t_1()
            .border_color(hsla(t.border))
            .child(SharedString::from(left))
            .child(SharedString::from(right))
    }

    /// What the canvas needs for one frame, gathered under one session lock.
    fn frame(&self, session: &LiveSession, size: GridSize) -> Frame {
        if let Err(e) = session.resize(size) {
            tracing::warn!("resize failed: {e:#}");
        }
        session.with(|p| {
            let e = p.engine();
            let snap = e.snapshot();
            let top = snap.top_line_abs();
            let bottom = top + snap.lines.len();
            let visible = |m: &Match| m.end.line >= top && m.start.line < bottom;
            let mut overlay = Overlay {
                selection: self.selection.filter(|s| !s.is_empty()).map(|s| s.range(e)),
                link: self.hover_link.as_ref().map(|l| (l.start, l.end)),
                ..Overlay::default()
            };
            if let Some(s) = &self.search {
                overlay.matches = s
                    .matches
                    .iter()
                    .filter(|m| visible(m))
                    .map(|m| (m.start, m.end))
                    .collect();
                overlay.current_match = s
                    .current
                    .and_then(|i| s.matches.get(i))
                    .map(|m| (m.start, m.end));
            }
            let blocks = p
                .blocks()
                .iter()
                .map(|b| BlockMark {
                    start: b.prompt_line,
                    end: b.end_line,
                    state: b.state,
                    exit_code: b.exit_code,
                })
                .collect();
            Frame {
                snap,
                blocks,
                overlay,
                hitbox: None,
            }
        })
    }
}

struct Frame {
    snap: Snapshot,
    blocks: Vec<BlockMark>,
    overlay: Overlay,
    hitbox: Option<Hitbox>,
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = Arc::clone(&self.theme);
        let metrics = self.metrics(window);
        let font = self.font.clone();
        let font_size = px(self.font_size);
        let focused = self.focus.is_focused(window);
        let session = self.session.clone();
        let grid = session.as_ref().map(|s| s.with(|p| p.engine().size()));
        let view: Entity<Self> = cx.entity();
        let mouse_mode = self.modes().mouse != MouseTracking::Off;
        let over_link = self.hover_link.is_some();

        let exit_overlay = self.exited().flatten().map(|code| {
            div()
                .absolute()
                .bottom(px(tokens::space::LG))
                .left(px(tokens::space::LG))
                .px(px(tokens::space::MD))
                .py(px(tokens::space::SM))
                .rounded(px(tokens::radius::LG))
                .bg(hsla(theme.bg_elevated))
                .border_1()
                .border_color(hsla(theme.border_strong))
                .text_color(hsla(theme.text_muted))
                .child(SharedString::from(format!(
                    "Process exited with code {code}. Press Enter to restart."
                )))
        });
        let error = self.error.clone().map(|e| {
            div()
                .p(px(tokens::space::XL))
                .text_color(hsla(theme.danger))
                .child(SharedString::from(format!(
                    "Could not start the shell: {e}"
                )))
        });

        let grid_canvas = canvas(
            {
                let view = view.clone();
                move |bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
                    let cols = ((bounds.size.width - px(crate::paint::PAD_X * 2.0))
                        / metrics.cell_w)
                        .floor();
                    let rows = ((bounds.size.height - px(crate::paint::PAD_Y * 2.0))
                        / metrics.line_h)
                        .floor();
                    let size = GridSize::new(cols.max(2.0) as u16, rows.max(1.0) as u16);
                    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                    let frame = session.as_ref().map(|s| view.read(cx).frame(s, size));
                    frame.map(|f| Frame {
                        hitbox: Some(hitbox),
                        ..f
                    })
                }
            },
            move |bounds, frame, window, cx| {
                let Some(frame) = frame else { return };
                view.update(cx, |v, _| v.grid_bounds = bounds);
                if let Some(hitbox) = &frame.hitbox {
                    let style = if over_link {
                        CursorStyle::PointingHand
                    } else if mouse_mode {
                        CursorStyle::Arrow
                    } else {
                        CursorStyle::IBeam
                    };
                    window.set_cursor_style(style, hitbox);
                }
                paint_grid(
                    PaintArgs {
                        snap: &frame.snap,
                        blocks: &frame.blocks,
                        overlay: &frame.overlay,
                        bounds,
                        theme: &theme,
                        font: &font,
                        font_size,
                        metrics,
                        focused,
                    },
                    window,
                    cx,
                );

                // Mouse listeners are window-level so drags keep working
                // outside the grid; presses only count when over it.
                let hitbox = frame.hitbox.clone();
                let v = view.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble
                        && hitbox.as_ref().is_some_and(|h| h.is_hovered(window))
                    {
                        v.update(cx, |this, cx| this.mouse_down(ev, window, cx));
                    }
                });
                let v = view.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        v.update(cx, |this, cx| this.mouse_move(ev, window, cx));
                    }
                });
                let v = view.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        v.update(cx, |this, cx| this.mouse_up(ev, window, cx));
                    }
                });
            },
        )
        .size_full();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(self.theme.term_bg))
            .font_family(self.font.family.clone())
            .text_size(px(tokens::text::SM))
            .child(self.header())
            .child(
                div()
                    .id("terminal")
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(Self::key_down))
                    .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                    .on_modifiers_changed(cx.listener(
                        |this, ev: &gpui::ModifiersChangedEvent, window, cx| {
                            let at = this.hit(window.mouse_position(), window).map(|(_, p)| p);
                            this.update_hover(ev.modifiers, at, cx);
                        },
                    ))
                    .child(grid_canvas)
                    .children(self.search_bar())
                    .children(error)
                    .children(exit_overlay),
            )
            .child(self.status_bar(grid))
    }
}
