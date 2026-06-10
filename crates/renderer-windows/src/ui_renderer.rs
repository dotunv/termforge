use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;

pub use crate::tokens::*;

/// Which member of the UI font set a piece of text is drawn with.  This is how
/// the redesign gets type hierarchy out of a renderer that otherwise had a
/// single 11pt weight-400 font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiTextStyle {
    /// 11pt / 400 — default body text.
    Body,
    /// 11pt / 600 — emphasised labels.
    Bold,
    /// ~13.5pt / 600 — card and pane titles.
    Title,
    /// ~9.5pt / 400 — captions, metadata, hints.
    Caption,
}

#[derive(Debug, Clone)]
pub enum UiCommand {
    FillRect { x: f32, y: f32, w: f32, h: f32, color: [f32; 4] },
    BottomBorder { x: f32, y: f32, w: f32, h: f32, color: [f32; 4] },
    DrawCircle { cx: f32, cy: f32, r: f32, fg: [f32; 4], bg: [f32; 4] },
    FillRoundRect { x: f32, y: f32, w: f32, h: f32, radius: f32, color: [f32; 4], bg: [f32; 4] },
    /// Outline (stroke-only) rounded rect — cards, pane borders, rings.
    StrokeRoundRect { x: f32, y: f32, w: f32, h: f32, radius: f32, thickness: f32, color: [f32; 4] },
    DrawText { x: f32, y: f32, text: String, fg: [f32; 4], bg: [f32; 4] },
    DrawUiText { x: f32, y: f32, text: String, fg: [f32; 4], bg: [f32; 4] },
    /// UI text drawn in a specific member of the font set (Body/Bold/Title/Caption).
    DrawStyledText { x: f32, y: f32, text: String, fg: [f32; 4], bg: [f32; 4], style: UiTextStyle },
    /// Icon glyph from the system icon font (Segoe Fluent Icons / MDL2 Assets).
    DrawIcon { x: f32, y: f32, text: String, fg: [f32; 4], bg: [f32; 4], large: bool },
    DropShadow { x: f32, y: f32, w: f32, h: f32, radius: f32, offset_x: f32, offset_y: f32, opacity: f32 },
}

/// The full set of GDI fonts the chrome renderer draws with, built once by the
/// compositor and borrowed per frame.
pub struct Fonts {
    pub mono: HFONT,
    pub ui_body: HFONT,
    pub ui_bold: HFONT,
    pub ui_title: HFONT,
    pub ui_caption: HFONT,
    pub icon_body: HFONT,
    pub icon_title: HFONT,
}

impl Fonts {
    fn ui(&self, style: UiTextStyle) -> HFONT {
        match style {
            UiTextStyle::Body => self.ui_body,
            UiTextStyle::Bold => self.ui_bold,
            UiTextStyle::Title => self.ui_title,
            UiTextStyle::Caption => self.ui_caption,
        }
    }
}

pub fn to_colorref(c: [f32; 4]) -> COLORREF {
    let r = (c[0].clamp(0.0, 1.0) * 255.0) as u32;
    let g = (c[1].clamp(0.0, 1.0) * 255.0) as u32;
    let b = (c[2].clamp(0.0, 1.0) * 255.0) as u32;
    COLORREF(b << 16 | g << 8 | r)
}

fn to_rect(x: f32, y: f32, w: f32, h: f32) -> RECT {
    RECT {
        left: x as i32,
        top: y as i32,
        right: (x + w) as i32,
        bottom: (y + h) as i32,
    }
}

// Cached 1×1 source surface for alpha fills: one DIB section + memory DC per
// thread, created on first use and reused for every translucent rect instead
// of allocating and destroying GDI objects per draw call.
struct AlphaSrc {
    dc: HDC,
    dib: HBITMAP,
    bits: *mut u32,
}

thread_local! {
    static ALPHA_SRC: std::cell::RefCell<Option<AlphaSrc>> =
        const { std::cell::RefCell::new(None) };
}

unsafe fn alpha_src_init(hdc: HDC) -> Option<AlphaSrc> {
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: 1,
            biHeight: 1,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(hdc, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    let dc = CreateCompatibleDC(hdc);
    if dc.is_invalid() {
        let _ = DeleteObject(dib);
        return None;
    }
    SelectObject(dc, dib);
    Some(AlphaSrc { dc, dib, bits: bits as *mut u32 })
}

/// Fill a rect honouring the colour's alpha channel.  Opaque colours take the
/// fast `FillRect` path; translucent ones AlphaBlend a cached 1×1
/// premultiplied DIB stretched over the target so dim overlays / soft accents
/// actually blend instead of rendering solid.
unsafe fn fill_rect_alpha(hdc: HDC, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
    let a = color[3].clamp(0.0, 1.0);
    if a <= 0.003 {
        return;
    }
    let rect = to_rect(x, y, w, h);
    if a >= 0.997 {
        let brush = CreateSolidBrush(to_colorref(color));
        FillRect(hdc, &rect, brush);
        let _ = DeleteObject(brush);
        return;
    }

    ALPHA_SRC.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = alpha_src_init(hdc);
        }
        let Some(src) = slot.as_ref() else { return };

        let alpha = (a * 255.0) as u32;
        let pr = (color[0].clamp(0.0, 1.0) * a * 255.0) as u32;
        let pg = (color[1].clamp(0.0, 1.0) * a * 255.0) as u32;
        let pb = (color[2].clamp(0.0, 1.0) * a * 255.0) as u32;
        *src.bits = alpha << 24 | pr << 16 | pg << 8 | pb;
        // Flush GDI batching so the pixel write is visible to AlphaBlend.
        let _ = GdiFlush();

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = AlphaBlend(
            hdc,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            src.dc,
            0,
            0,
            1,
            1,
            blend,
        );
        let _ = src.dib; // kept selected in src.dc for the thread's lifetime
    });
}

fn draw_text_internal(
    hdc: HDC,
    font: HFONT,
    x: f32,
    y: f32,
    text: &str,
    fg: [f32; 4],
    bg: [f32; 4],
    opaque: bool,
) {
    unsafe {
        let old_font = SelectObject(hdc, font);
        SetTextColor(hdc, to_colorref(fg));
        SetBkColor(hdc, to_colorref(bg));
        SetBkMode(hdc, if opaque { OPAQUE } else { TRANSPARENT });

        let wide: Vec<u16> = text.encode_utf16().collect();
        let len = wide.len() as u32;
        let flags = if opaque { ETO_CLIPPED | ETO_OPAQUE } else { ETO_CLIPPED };

        let _ = ExtTextOutW(
            hdc,
            x as i32,
            y as i32,
            flags,
            None,
            windows::core::PCWSTR(wide.as_ptr()),
            len,
            None,
        );
        SelectObject(hdc, old_font);
    }
}

/// Stroke (outline) a rounded rectangle with a geometric pen of `thickness`.
unsafe fn stroke_round_rect(hdc: HDC, x: f32, y: f32, w: f32, h: f32, radius: f32, thickness: f32, color: [f32; 4]) {
    let pen = CreatePen(PS_SOLID, thickness.max(1.0) as i32, to_colorref(color));
    let null_brush = GetStockObject(NULL_BRUSH);
    let old_pen = SelectObject(hdc, pen);
    let old_brush = SelectObject(hdc, null_brush);
    let d = (radius * 2.0) as i32;
    let _ = RoundRect(hdc, x as i32, y as i32, (x + w) as i32, (y + h) as i32, d, d);
    SelectObject(hdc, old_pen);
    SelectObject(hdc, old_brush);
    let _ = DeleteObject(pen);
}

/// # Safety
/// `hdc` must be a valid device context and every handle in `fonts` must be a
/// valid GDI font for the duration of the call.
pub unsafe fn draw_command(hdc: HDC, cmd: &UiCommand, fonts: &Fonts) {
    match cmd {
        UiCommand::FillRect { x, y, w, h, color } => {
            fill_rect_alpha(hdc, *x, *y, *w, *h, *color);
        }
        UiCommand::BottomBorder { x, y, w, h, color } => {
            fill_rect_alpha(hdc, *x, *y + *h - 1.0, *w, 1.0, *color);
        }
        UiCommand::DrawCircle { cx, cy, r, fg, bg } => {
            let brush = CreateSolidBrush(to_colorref(*fg));
            let pen = CreatePen(PS_SOLID, 1, to_colorref(*fg));
            // GDI objects must be deselected from the DC before deletion,
            // otherwise DeleteObject fails and the handle leaks every frame.
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, pen);
            let _ = bg; // backing colour is already painted by the caller
            let left = (*cx - *r) as i32;
            let top = (*cy - *r) as i32;
            let right = (*cx + *r) as i32;
            let bottom = (*cy + *r) as i32;
            let _ = Ellipse(hdc, left, top, right, bottom);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush);
            let _ = DeleteObject(pen);
        }
        UiCommand::FillRoundRect { x, y, w, h, radius, color, bg } => {
            let brush = CreateSolidBrush(to_colorref(*color));
            let pen = CreatePen(PS_SOLID, 1, to_colorref(*color));
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, pen);
            let _ = bg;
            let d = (*radius * 2.0) as i32;
            let _ = RoundRect(hdc, *x as i32, *y as i32, (*x + *w) as i32, (*y + *h) as i32, d, d);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush);
            let _ = DeleteObject(pen);
        }
        UiCommand::StrokeRoundRect { x, y, w, h, radius, thickness, color } => {
            stroke_round_rect(hdc, *x, *y, *w, *h, *radius, *thickness, *color);
        }
        UiCommand::DrawText { x, y, text, fg, bg } => {
            draw_text_internal(hdc, fonts.mono, *x, *y, text, *fg, *bg, true);
        }
        UiCommand::DrawUiText { x, y, text, fg, bg } => {
            draw_text_internal(hdc, fonts.ui_body, *x, *y, text, *fg, *bg, true);
        }
        UiCommand::DrawStyledText { x, y, text, fg, bg, style } => {
            draw_text_internal(hdc, fonts.ui(*style), *x, *y, text, *fg, *bg, true);
        }
        UiCommand::DrawIcon { x, y, text, fg, bg, large } => {
            // Icons are drawn transparently so they sit cleanly over whatever
            // backplate the caller already painted.
            let font = if *large { fonts.icon_title } else { fonts.icon_body };
            draw_text_internal(hdc, font, *x, *y, text, *fg, *bg, false);
        }
        UiCommand::DropShadow { x, y, w, h, radius, offset_x: ox, offset_y: oy, opacity } => {
            // Soft shadow approximated by stacking translucent rects that grow
            // outward with decaying alpha — a convincing blur without a DIB
            // box-blur pass.
            let layers = 6u32;
            let spread = radius.max(4.0);
            for i in 0..layers {
                let t = i as f32 / layers as f32;
                let grow = spread * t;
                let a = opacity * (1.0 - t) * 0.5;
                fill_rect_alpha(
                    hdc,
                    *x + *ox - grow,
                    *y + *oy - grow,
                    *w + grow * 2.0,
                    *h + grow * 2.0,
                    [0.0, 0.0, 0.0, a],
                );
            }
        }
    }
}

/// # Safety
/// Same contract as [`draw_command`]: `hdc` must be valid and `fonts` must hold
/// valid GDI font handles for the duration of the call.
pub unsafe fn draw_commands(hdc: HDC, cmds: &[UiCommand], fonts: &Fonts) {
    for cmd in cmds {
        draw_command(hdc, cmd, fonts);
    }
}
