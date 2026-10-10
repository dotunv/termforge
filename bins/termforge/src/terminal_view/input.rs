//! Keyboard, mouse, scroll and selection handling.

use super::*;

impl TerminalView {
    pub(super) fn metrics(&self, window: &Window) -> Metrics {
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

    pub(super) fn write(&self, bytes: &[u8]) {
        if let Some(s) = &self.session {
            if let Err(e) = s.write(bytes) {
                tracing::warn!("write to pty failed: {e:#}");
            }
        }
    }

    pub(super) fn modes(&self) -> tf_engine::Modes {
        self.session
            .as_ref()
            .map(|s| s.with(|p| p.engine().modes()))
            .unwrap_or_default()
    }

    pub(super) fn input_modes(&self) -> InputModes {
        let m = self.modes();
        InputModes {
            app_cursor: m.app_cursor,
            bracketed_paste: m.bracketed_paste,
        }
    }

    pub(super) fn exited(&self) -> Option<Option<u32>> {
        self.session.as_ref().and_then(|s| s.exit_status())
    }

    pub(super) fn focus_changed(&mut self, focused: bool, cx: &mut Context<'_, Self>) {
        if self.modes().focus_events {
            self.write(encode_focus(focused));
        }
        cx.notify();
    }

    // ---- keyboard -------------------------------------------------------

    pub(super) fn key_down(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let mods = Mods {
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
        };
        let key = ks.key.as_str();
        let ctrl_shift = m.control && m.shift && !m.alt;

        if self.shell_menu && key == "escape" {
            self.shell_menu = false;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.task_form_key(ev, cx) {
            cx.stop_propagation();
            return;
        }
        if self.renaming.is_some() {
            self.rename_key(ev, cx);
            cx.stop_propagation();
            return;
        }
        if self.settings.is_some() {
            if key == "escape" {
                self.settings = None;
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if self.search.is_some() && self.search_key(ev, cx) {
            cx.stop_propagation();
            return;
        }
        if self.palette.is_some() && self.palette_key(ev, cx) {
            cx.stop_propagation();
            return;
        }

        // App shortcuts first. Ctrl+Shift chords never collide with shells.
        if ctrl_shift && key.eq_ignore_ascii_case("t") {
            self.new_local_tab(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("d") {
            self.split_pane(Axis::Horizontal, cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("e") {
            self.split_pane(Axis::Vertical, cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("x") {
            self.close_pane(cx);
        } else if m.control && m.alt && m.shift && matches!(key, "left" | "right" | "up" | "down") {
            let dir = match key {
                "left" => Dir::Left,
                "right" => Dir::Right,
                "up" => Dir::Up,
                _ => Dir::Down,
            };
            self.resize_pane(dir, cx);
        } else if m.control && m.alt && !m.shift && matches!(key, "left" | "right" | "up" | "down")
        {
            let dir = match key {
                "left" => Dir::Left,
                "right" => Dir::Right,
                "up" => Dir::Up,
                _ => Dir::Down,
            };
            self.focus_direction(dir, cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("w") {
            self.close_active_tab(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("q") {
            cx.quit();
        } else if ctrl_shift && key.eq_ignore_ascii_case("b") {
            self.toggle_sidebar(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("u") {
            self.jump_to_attention(cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("r") {
            self.begin_rename(self.active_tab, cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("k") {
            self.toggle_pin(self.active_tab, cx);
        } else if ctrl_shift && matches!(key, "up" | "down") {
            self.move_active_tab(key == "down", cx);
        } else if m.control && !m.shift && !m.alt && key == "," {
            self.open_settings(cx);
        } else if m.control && key == "tab" {
            self.cycle_tab(m.shift, cx);
        } else if ctrl_shift && key.eq_ignore_ascii_case("p") {
            self.ssh_hosts = tf_pty::discover_ssh_hosts();
            self.palette = Some(PaletteState::default());
            cx.notify();
        } else if ctrl_shift && key.eq_ignore_ascii_case("f") {
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
        } else if m.control
            && !m.shift
            && !m.alt
            && key.len() == 1
            && key.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            // Ctrl+1..Ctrl+9 for direct tab access
            if let Some(digit) = key.chars().next().and_then(|c| c.to_digit(10)) {
                let index = (digit as usize).saturating_sub(1);
                if index < self.tabs.len() {
                    self.select_tab(index, cx);
                }
            }
        } else if m.control && m.shift && !m.alt && matches!(key, "left" | "right") {
            // Ctrl+Shift+Left/Right for tab navigation
            self.cycle_tab(key == "left", cx);
        } else {
            if key == "enter" && !m.control && !m.alt && !m.shift {
                self.note_enter();
            }
            self.send_key(ev, mods, window, cx);
            return;
        }
        cx.stop_propagation();
    }

    pub(super) fn send_key(
        &mut self,
        ev: &KeyDownEvent,
        mods: Mods,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let ks = &ev.keystroke;
        let key = ks.key.as_str();
        if self.exited().is_some() {
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

    pub(super) fn paste(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let bytes = encode_paste(&text, self.input_modes());
            self.selection = None;
            self.scroll(Scroll::Bottom, cx);
            self.write(&bytes);
        }
    }

    pub(super) fn scroll(&mut self, scroll: Scroll, cx: &mut Context<'_, Self>) {
        if let Some(s) = &self.session {
            s.with_mut(|p| p.engine_mut().scroll(scroll));
            cx.notify();
        }
    }

    // ---- selection ------------------------------------------------------

    pub(super) fn has_selection(&self) -> bool {
        self.selection.is_some_and(|s| !s.is_empty())
    }

    pub(super) fn selection_text(&self) -> Option<String> {
        let sel = self.selection.filter(|s| !s.is_empty())?;
        let text = self.session.as_ref()?.with(|p| sel.text(p.engine()));
        (!text.is_empty()).then_some(text)
    }

    pub(super) fn copy_selection(&self, cx: &mut Context<'_, Self>) {
        if let Some(text) = self.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn clear_selection(&mut self, cx: &mut Context<'_, Self>) {
        self.selection = None;
        cx.notify();
    }

    pub(super) fn select_all(&mut self, cx: &mut Context<'_, Self>) {
        let Some(s) = &self.session else { return };
        let (total, cols) = s.with(|p| (p.engine().total_lines(), p.engine().size().cols));
        let mut sel = Selection::new(tf_engine::Point::new(0, 0), SelectionKind::Cells);
        sel.extend(tf_engine::Point::new(
            total.saturating_sub(1),
            cols.saturating_sub(1),
        ));
        self.selection = Some(sel);
        cx.notify();
    }

    // ---- mouse ----------------------------------------------------------

    /// Viewport cell and absolute point under a window position.
    pub(super) fn hit(
        &self,
        pos: gpui::Point<Pixels>,
        window: &Window,
    ) -> Option<((u16, u16), tf_engine::Point)> {
        let s = self.session.as_ref()?;
        let (size, top) = s.with(|p| {
            let e = p.engine();
            (e.size(), e.history_size() - e.display_offset())
        });
        let cell = self
            .metrics(window)
            .cell_at(self.grid_bounds, pos, size.cols, size.rows);
        Some((cell, tf_engine::Point::new(top + cell.1 as usize, cell.0)))
    }

    /// Mouse tracking applies unless Shift is held (Shift always selects).
    pub(super) fn tracking(&self, mods: Modifiers) -> Tracking {
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

    pub(super) fn report(
        &mut self,
        action: MouseAction,
        cell: (u16, u16),
        mods: Modifiers,
    ) -> bool {
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

    pub(super) fn mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
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

    pub(super) fn mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
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

    pub(super) fn mouse_up(
        &mut self,
        ev: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
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

    pub(super) fn link_at(&self, at: tf_engine::Point) -> Option<Link> {
        self.session.as_ref()?.with(|p| link_at(p.engine(), at))
    }

    /// Ctrl+hover underlines links, as in VS Code and Windows Terminal.
    pub(super) fn update_hover(
        &mut self,
        mods: Modifiers,
        at: Option<tf_engine::Point>,
        cx: &mut Context<'_, Self>,
    ) {
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

    pub(super) fn scroll_wheel(
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
}
