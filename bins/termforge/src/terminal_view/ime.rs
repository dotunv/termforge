//! Native IME bridge.
//!
//! A terminal cannot edit the shell's input buffer itself. We therefore keep
//! only the platform's transient preedit text locally and forward committed
//! text as UTF-8 bytes. This prevents candidate updates from being typed into
//! the shell multiple times while still giving the OS a caret rectangle for
//! its candidate window.

use std::ops::Range;

use gpui::{point, size, Bounds, Context, EntityInputHandler, Pixels, UTF16Selection, Window};

use super::*;

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<'_, Self>,
    ) -> Option<String> {
        let utf16: Vec<u16> = self.ime_preedit.encode_utf16().collect();
        let start = range.start.min(utf16.len());
        let end = range.end.min(utf16.len()).max(start);
        *adjusted_range = Some(start..end);
        Some(String::from_utf16_lossy(&utf16[start..end]))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<'_, Self>,
    ) -> Option<UTF16Selection> {
        let end = self.ime_preedit.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<'_, Self>,
    ) -> Option<Range<usize>> {
        (!self.ime_preedit.is_empty()).then(|| 0..self.ime_preedit.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) {
        self.ime_preedit.clear();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.ime_preedit.clear();
        if !text.is_empty() {
            if self.workspace_artifact == WorkspaceArtifact::Files && !self.dock_focus {
                if let Some(buffer) = self.editor_buffer.as_mut() {
                    buffer.replace_selection(text);
                    self.editor_error = None;
                }
                self.sync_editor_lsp();
                self.reveal_editor_selection();
            } else {
                self.selection = None;
                self.scroll(Scroll::Bottom, cx);
                self.write(text.as_bytes());
            }
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.ime_preedit.clear();
        self.ime_preedit.push_str(new_text);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<'_, Self>,
    ) -> Option<Bounds<Pixels>> {
        let metrics = self.metrics(window);
        let cursor = self
            .session
            .as_ref()
            .map(|session| session.with(|processor| processor.engine().snapshot().cursor))?;
        Some(Bounds::new(
            point(
                element_bounds.left()
                    + px(crate::paint::PAD_X)
                    + metrics.cell_w * cursor.col as f32,
                element_bounds.top() + px(crate::paint::PAD_Y) + metrics.line_h * cursor.row as f32,
            ),
            size(metrics.cell_w, metrics.line_h),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<'_, Self>,
    ) -> Option<usize> {
        Some(self.ime_preedit.encode_utf16().count())
    }
}
