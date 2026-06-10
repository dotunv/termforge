use anyhow::Result;
use libterm::grid::cell::Color;
use libterm::mux::layout::PaneLayout;
use libterm::mux::session::Session;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;

use crate::tokens::{self, BG_BASE};
use crate::ui_renderer::{self, to_colorref, Fonts, UiCommand};

// ── ANSI 256-colour palette ───────────────────────────────────────────────────

static ANSI_256: [[u8; 3]; 256] = {
    let mut t = [[0u8; 3]; 256];
    t[0]  = [0,   0,   0  ]; t[1]  = [128, 0,   0  ];
    t[2]  = [0,   128, 0  ]; t[3]  = [128, 128, 0  ];
    t[4]  = [0,   0,   128]; t[5]  = [128, 0,   128];
    t[6]  = [0,   128, 128]; t[7]  = [192, 192, 192];
    t[8]  = [128, 128, 128]; t[9]  = [255, 0,   0  ];
    t[10] = [0,   255, 0  ]; t[11] = [255, 255, 0  ];
    t[12] = [0,   0,   255]; t[13] = [255, 0,   255];
    t[14] = [0,   255, 255]; t[15] = [255, 255, 255];
    let mut i = 16usize;
    let mut r = 0usize;
    while r < 6 {
        let mut g = 0usize;
        while g < 6 {
            let mut b = 0usize;
            while b < 6 {
                let rv = if r == 0 { 0 } else { 55 + r as u8 * 40 };
                let gv = if g == 0 { 0 } else { 55 + g as u8 * 40 };
                let bv = if b == 0 { 0 } else { 55 + b as u8 * 40 };
                t[i] = [rv, gv, bv];
                i += 1; b += 1;
            }
            g += 1;
        }
        r += 1;
    }
    let mut k = 0usize;
    while k < 24 {
        t[232 + k] = [8 + k as u8 * 10; 3];
        k += 1;
    }
    t
};

// ── Colour resolution ─────────────────────────────────────────────────────────

fn resolve_fg(color: &Color) -> [f32; 4] {
    match color {
        Color::Default => tokens::TEXT_PRIMARY,
        Color::Rgb(r, g, b) => [*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0, 1.0],
        Color::Indexed(i) => {
            let [r, g, b] = ANSI_256[*i as usize];
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
        }
    }
}

fn resolve_bg(color: &Color) -> [f32; 4] {
    match color {
        Color::Default => BG_BASE,
        Color::Rgb(r, g, b) => [*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0, 1.0],
        Color::Indexed(i) => {
            let [r, g, b] = ANSI_256[*i as usize];
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
        }
    }
}

// ── Font helpers ──────────────────────────────────────────────────────────────

unsafe fn create_font(name: &str, size_pt: f32, weight: u32, monospace: bool, dpi: f32) -> Result<HFONT> {
    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut lf = LOGFONTW::default();
    // Points → device pixels at the monitor's actual DPI (96 = 100 %).
    // Hardcoding 96 renders undersized glyphs that DWM then scales up blurry
    // on high-DPI displays.
    lf.lfHeight = -(size_pt * dpi / 72.0).round() as i32;
    lf.lfWeight = weight as i32;
    lf.lfCharSet = DEFAULT_CHARSET;
    lf.lfOutPrecision = OUT_DEFAULT_PRECIS;
    lf.lfClipPrecision = CLIP_DEFAULT_PRECIS;
    lf.lfQuality = CLEARTYPE_QUALITY;
    lf.lfPitchAndFamily = if monospace {
        FIXED_PITCH.0 | FF_DONTCARE.0
    } else {
        VARIABLE_PITCH.0 | FF_DONTCARE.0
    };
    let mut dst = lf.lfFaceName.iter_mut();
    for &c in &wide_name {
        if let Some(slot) = dst.next() {
            *slot = c;
        }
    }

    let font = CreateFontIndirectW(&lf);
    if font.is_invalid() {
        anyhow::bail!("CreateFontIndirectW failed for {name}");
    }
    Ok(font)
}

/// Build the icon font from Windows' system icon set.  Segoe Fluent Icons ships
/// on Windows 11; Segoe MDL2 Assets is the Windows 10 fallback.  Both expose the
/// same core PUA codepoints used by [`crate::icons`].
unsafe fn create_icon_font(size_pt: f32, dpi: f32) -> Result<HFONT> {
    create_font("Segoe Fluent Icons", size_pt, 400, false, dpi)
        .or_else(|_| create_font("Segoe MDL2 Assets", size_pt, 400, false, dpi))
}

/// Build the full UI font set at the given DPI.
unsafe fn build_ui_fonts(dpi: f32) -> Result<UiFonts> {
    Ok(UiFonts {
        ui_body: create_font("Segoe UI", tokens::UI_PT_BODY, tokens::UI_WEIGHT_REGULAR, false, dpi)?,
        ui_bold: create_font("Segoe UI", tokens::UI_PT_BODY, tokens::UI_WEIGHT_SEMIBOLD, false, dpi)?,
        ui_title: create_font("Segoe UI", tokens::UI_PT_TITLE, tokens::UI_WEIGHT_SEMIBOLD, false, dpi)?,
        ui_caption: create_font("Segoe UI", tokens::UI_PT_CAPTION, tokens::UI_WEIGHT_REGULAR, false, dpi)?,
        icon_body: create_icon_font(tokens::ICON_PT_BODY, dpi)?,
        icon_title: create_icon_font(tokens::ICON_PT_TITLE, dpi)?,
    })
}

/// The UI font set (everything except the terminal mono font, which the
/// compositor owns separately for grid rendering).
struct UiFonts {
    ui_body: HFONT,
    ui_bold: HFONT,
    ui_title: HFONT,
    ui_caption: HFONT,
    icon_body: HFONT,
    icon_title: HFONT,
}

impl UiFonts {
    unsafe fn destroy(&self) {
        for f in [self.ui_body, self.ui_bold, self.ui_title, self.ui_caption, self.icon_body, self.icon_title] {
            if !f.is_invalid() {
                let _ = DeleteObject(f);
            }
        }
    }
}

// ── Compositor ─────────────────────────────────────────────────────────────────

pub struct Compositor {
    hwnd: HWND,
    pub cell_w: u32,
    pub cell_h: u32,
    pub ui_char_w: f32,
    mono_font: HFONT,
    ui: UiFonts,
    cursor_blink_on: bool,
    last_blink: std::time::Instant,
    window_w: u32,
    window_h: u32,
    mem_dc: Option<HDC>,
    mem_bmp: Option<HBITMAP>,
    old_bmp: Option<HGDIOBJ>,
    last_bmp_w: u32,
    last_bmp_h: u32,
}

impl Compositor {
    pub fn build(hwnd: HWND, font_family: &str, font_size_pt: f32, dpi: f32) -> Result<Self> {
        unsafe {
            let dpi = if dpi > 0.0 { dpi } else { 96.0 };
            let mono_font = create_font(font_family, font_size_pt, 400, true, dpi)?;
            let ui = build_ui_fonts(dpi)?;

            let dc = GetDC(hwnd);
            if dc.is_invalid() {
                anyhow::bail!("GetDC failed");
            }
            SelectObject(dc, mono_font);
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(dc, &mut tm);
            let cell_w = tm.tmAveCharWidth as u32;
            let cell_h = (tm.tmHeight + tm.tmExternalLeading) as u32;

            SelectObject(dc, ui.ui_body);
            let mut ui_tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(dc, &mut ui_tm);
            let ui_char_w = ui_tm.tmAveCharWidth as f32;

            ReleaseDC(hwnd, dc);

            Ok(Self {
                hwnd,
                cell_w: cell_w.max(1),
                cell_h: cell_h.max(1),
                ui_char_w,
                mono_font,
                ui,
                cursor_blink_on: true,
                last_blink: std::time::Instant::now(),
                window_w: 0,
                window_h: 0,
                mem_dc: None,
                mem_bmp: None,
                old_bmp: None,
                last_bmp_w: 0,
                last_bmp_h: 0,
            })
        }
    }

    pub fn cell_size(&self) -> (u32, u32) {
        (self.cell_w, self.cell_h)
    }

    pub fn ui_char_w(&self) -> f32 {
        self.ui_char_w
    }

    pub fn measure_ui_text(&self, text: &str) -> f32 {
        self.measure_ui_text_style(text, ui_renderer::UiTextStyle::Body)
    }

    /// Measure UI text in a specific font-set member, so width-dependent layout
    /// (tabs, badges, hints) stays correct when text is drawn larger/bolder.
    pub fn measure_ui_text_style(&self, text: &str, style: ui_renderer::UiTextStyle) -> f32 {
        unsafe {
            let font = match style {
                ui_renderer::UiTextStyle::Body => self.ui.ui_body,
                ui_renderer::UiTextStyle::Bold => self.ui.ui_bold,
                ui_renderer::UiTextStyle::Title => self.ui.ui_title,
                ui_renderer::UiTextStyle::Caption => self.ui.ui_caption,
            };
            let dc = GetDC(self.hwnd);
            if dc.is_invalid() {
                return text.len() as f32 * self.ui_char_w;
            }
            SelectObject(dc, font);
            let wide: Vec<u16> = text.encode_utf16().collect();
            let mut sz = SIZE { cx: 0, cy: 0 };
            let _ = GetTextExtentPoint32W(dc, &wide, &mut sz);
            ReleaseDC(self.hwnd, dc);
            sz.cx as f32
        }
    }

    pub fn cursor_blink_due(&mut self) -> bool {
        if self.last_blink.elapsed() >= std::time::Duration::from_millis(500) {
            self.cursor_blink_on = !self.cursor_blink_on;
            self.last_blink = std::time::Instant::now();
            true
        } else {
            false
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.window_w = width;
        self.window_h = height;
        Ok(())
    }

    pub fn rebuild_atlas(&mut self, font_family: &str, font_size_pt: f32, dpi: f32) -> Result<()> {
        let dpi = if dpi > 0.0 { dpi } else { 96.0 };
        unsafe {
            if !self.mono_font.is_invalid() {
                let _ = DeleteObject(self.mono_font);
            }
            self.ui.destroy();
        }
        let mono_font = unsafe { create_font(font_family, font_size_pt, 400, true, dpi) }?;
        let ui = unsafe { build_ui_fonts(dpi) }?;
        self.mono_font = mono_font;
        self.ui = ui;

        unsafe {
            let dc = GetDC(self.hwnd);
            if !dc.is_invalid() {
                SelectObject(dc, self.mono_font);
                let mut tm = TEXTMETRICW::default();
                let _ = GetTextMetricsW(dc, &mut tm);
                self.cell_w = tm.tmAveCharWidth as u32;
                self.cell_h = (tm.tmHeight + tm.tmExternalLeading) as u32;
                // Refresh the UI font's average width too — leaving it stale
                // skews tab-width hit testing after a DPI change.
                SelectObject(dc, self.ui.ui_body);
                let mut ui_tm = TEXTMETRICW::default();
                let _ = GetTextMetricsW(dc, &mut ui_tm);
                self.ui_char_w = ui_tm.tmAveCharWidth as f32;
                ReleaseDC(self.hwnd, dc);
            }
        }
        Ok(())
    }

    /// Re-blit the last rendered frame to the window.  Used for WM_PAINT so
    /// the window restores content after being occluded or captured, without
    /// rebuilding the whole frame.  Returns false when no frame exists yet.
    pub fn present(&self) -> bool {
        unsafe {
            let Some(mem_dc) = self.mem_dc else { return false };
            if self.last_bmp_w == 0 || self.last_bmp_h == 0 {
                return false;
            }
            let wnd_dc = GetDC(self.hwnd);
            if wnd_dc.is_invalid() {
                return false;
            }
            let _ = BitBlt(
                wnd_dc,
                0, 0,
                self.last_bmp_w as i32, self.last_bmp_h as i32,
                mem_dc, 0, 0, SRCCOPY,
            );
            ReleaseDC(self.hwnd, wnd_dc);
            true
        }
    }

    /// Ensure the off-screen buffer is sized for `(w, h)`.
    unsafe fn ensure_buffer(&mut self, w: u32, h: u32) -> Result<()> {
        if self.last_bmp_w == w && self.last_bmp_h == h && self.mem_dc.is_some() {
            return Ok(());
        }
        self.destroy_buffer();

        let dc = GetDC(self.hwnd);
        if dc.is_invalid() {
            anyhow::bail!("GetDC failed");
        }
        let mem_dc = CreateCompatibleDC(dc);
        if mem_dc.is_invalid() {
            ReleaseDC(self.hwnd, dc);
            anyhow::bail!("CreateCompatibleDC failed");
        }
        let bmp = CreateCompatibleBitmap(dc, w as i32, h as i32);
        if bmp.is_invalid() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(self.hwnd, dc);
            anyhow::bail!("CreateCompatibleBitmap failed");
        }
        let old = SelectObject(mem_dc, bmp);
        ReleaseDC(self.hwnd, dc);

        self.mem_dc = Some(mem_dc);
        self.mem_bmp = Some(bmp);
        self.old_bmp = Some(old);
        self.last_bmp_w = w;
        self.last_bmp_h = h;
        Ok(())
    }

    unsafe fn destroy_buffer(&mut self) {
        if let Some(bmp) = self.mem_bmp.take() {
            if let Some(dc) = self.mem_dc {
                if let Some(old) = self.old_bmp {
                    SelectObject(dc, old);
                }
                let _ = DeleteDC(dc);
            }
            let _ = DeleteObject(bmp);
        }
        self.mem_dc = None;
        self.old_bmp = None;
        self.last_bmp_w = 0;
        self.last_bmp_h = 0;
    }

    // ── Cell-row rendering ────────────────────────────────────────────────

    /// Render a single terminal grid into the given DC at pixel position
    /// `(origin_x, origin_y)` with width `pane_w` and height `pane_h`.
    unsafe fn render_grid_to_dc(
        &self,
        hdc: HDC,
        grid: &libterm::grid::TerminalGrid,
        origin_x: f32,
        origin_y: f32,
        pane_w: f32,
        pane_h: f32,
        cursor_visible: bool,
        cursor_col: u16,
        cursor_row: u16,
    ) {
        let cw = self.cell_w as f32;
        let ch = self.cell_h as f32;
        let cols = grid.cols() as usize;
        let rows = grid.rows() as usize;

        let visible_cols = (pane_w / cw) as usize;
        let visible_rows = (pane_h / ch) as usize;

        SelectObject(hdc, self.mono_font);

        for row in 0..visible_rows.min(rows) {
            let y = origin_y + row as f32 * ch;

            struct Run {
                start: usize,
                end: usize,
                fg: [f32; 4],
                bg: [f32; 4],
            }
            let mut runs: Vec<Run> = Vec::new();

            for col in 0..visible_cols.min(cols) {
                let Some(cell) = grid.cell(col as u16, row as u16) else { continue };

                let fg = resolve_fg(&cell.fg);
                let bg = resolve_bg(&cell.bg);

                let (fg, bg) = if cursor_visible
                    && col == cursor_col as usize
                    && row == cursor_row as usize
                {
                    (bg, fg)
                } else {
                    (fg, bg)
                };

                let same = runs.last().is_some_and(|r| r.fg == fg && r.bg == bg);
                if same {
                    if let Some(r) = runs.last_mut() {
                        r.end = col + 1;
                    }
                } else {
                    runs.push(Run { start: col, end: col + 1, fg, bg });
                }
            }

            for run in &runs {
                let x = origin_x + run.start as f32 * cw;
                let run_w = (run.end - run.start) as f32 * cw;

                let mut text_utf16 = Vec::new();
                for col in run.start..run.end {
                    if let Some(cell) = grid.cell(col as u16, row as u16) {
                        let c = if cell.ch.is_control() || cell.ch == '\0' {
                            ' '
                        } else {
                            cell.ch
                        };
                        text_utf16.push(c as u16);
                    } else {
                        text_utf16.push(b' ' as u16);
                    }
                }

                SetTextColor(hdc, to_colorref(run.fg));
                SetBkColor(hdc, to_colorref(run.bg));
                SetBkMode(hdc, OPAQUE);

                let clip = RECT {
                    left: x as i32,
                    top: y as i32,
                    right: (x + run_w) as i32,
                    bottom: (y + ch) as i32,
                };

                let _ = ExtTextOutW(
                    hdc,
                    x as i32,
                    y as i32,
                    ETO_CLIPPED | ETO_OPAQUE,
                    Some(&clip as *const _),
                    windows::core::PCWSTR(text_utf16.as_ptr()),
                    text_utf16.len() as u32,
                    None,
                );
            }
        }
    }

    // ── Main render entry-point ───────────────────────────────────────────

    pub fn render_frame_with_chrome(
        &mut self,
        sessions: &[Session],
        layout: &PaneLayout,
        chrome_cmds: &[UiCommand],
        window_w: u32,
        window_h: u32,
        term_area: Option<(f32, f32, f32, f32)>,
    ) -> Result<()> {
        unsafe {
            let cursor_visible = self.cursor_blink_on;
            let active_session_id = sessions.first().map(|s| s.id);

            self.ensure_buffer(window_w, window_h)?;
            let Some(hdc) = self.mem_dc else {
                return Ok(());
            };

            // 1. Clear background.
            let bg_brush = CreateSolidBrush(to_colorref(BG_BASE));
            let full_rect = RECT { left: 0, top: 0, right: window_w as i32, bottom: window_h as i32 };
            let _ = FillRect(hdc, &full_rect, bg_brush);
            let _ = DeleteObject(bg_brush);

            // 2. Render terminal panes.
            let (term_x, term_y, term_w, term_h) =
                term_area.unwrap_or((0.0, 0.0, window_w as f32, window_h as f32));
            let pane_rects = layout.rects(term_x, term_y, term_w, term_h);

            for (session_id, px, py, pw, ph) in &pane_rects {
                let Some(session) = sessions.iter().find(|s| s.id == *session_id) else {
                    continue;
                };
                let pane_cursor = cursor_visible && (Some(session.id) == active_session_id);
                let (cursor_col, cursor_row) = if pane_cursor {
                    session.grid.cursor()
                } else {
                    (u16::MAX, u16::MAX)
                };

                self.render_grid_to_dc(
                    hdc, &session.grid, *px, *py, *pw, *ph,
                    pane_cursor, cursor_col, cursor_row,
                );
            }

            // 3. Draw chrome overlay.
            let fonts = Fonts {
                mono: self.mono_font,
                ui_body: self.ui.ui_body,
                ui_bold: self.ui.ui_bold,
                ui_title: self.ui.ui_title,
                ui_caption: self.ui.ui_caption,
                icon_body: self.ui.icon_body,
                icon_title: self.ui.icon_title,
            };
            ui_renderer::draw_commands(hdc, chrome_cmds, &fonts);

            // 4. BitBlt to window.
            let wnd_dc = GetDC(self.hwnd);
            if !wnd_dc.is_invalid() {
                let _ = BitBlt(
                    wnd_dc,
                    0, 0, window_w as i32, window_h as i32,
                    hdc, 0, 0, SRCCOPY,
                );
                ReleaseDC(self.hwnd, wnd_dc);
            }

            // 5. Clear session dirty flags.
            for session in sessions {
                session.clear_dirty();
            }

            Ok(())
        }
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        unsafe {
            self.destroy_buffer();
            if !self.mono_font.is_invalid() {
                let _ = DeleteObject(self.mono_font);
            }
            self.ui.destroy();
        }
    }
}
