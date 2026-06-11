//! Chrome command generation — orchestrates all sub-renderers into a single
//! flat `Vec<UiCommand>` that the compositor draws each frame.

use libterm::mux::session::SessionKind;
use renderer_windows::ui_renderer::{UiCommand, UiTextStyle};
use renderer_windows::tokens::*;

use super::layout::{ChromeState, Rect, CAPTION_BTN_W, CAPTION_ZONE_W};
use super::sidebar;

pub use super::layout::{
    PANE_HEADER_H, SPLIT_HANDLE_W,
};

/// Returns `true` when the mouse cursor is within the given rect.
fn hit_test(s: &ChromeState<'_>, x: f32, y: f32, w: f32, h: f32) -> bool {
    s.mouse_pos.0 >= 0.0
        && s.mouse_pos.0 >= x && s.mouse_pos.0 < x + w
        && s.mouse_pos.1 >= y && s.mouse_pos.1 < y + h
}

/// Returns the hover background colour when the mouse is inside the rect,
/// otherwise returns the default surface colour.
fn hover_or(s: &ChromeState<'_>, x: f32, y: f32, w: f32, h: f32, default: [f32; 4]) -> [f32; 4] {
    if hit_test(s, x, y, w, h) { BG_HOVER } else { default }
}

/// Display labels for the tab bar / sidebar.  Duplicate titles (three tabs
/// all called "local") get a numeric suffix so they're distinguishable.
/// Shared by the renderers and the click hit-test so widths stay in sync.
pub fn display_labels<'a>(titles: impl Iterator<Item = &'a str> + Clone) -> Vec<String> {
    use std::collections::HashMap;
    let mut total: HashMap<&str, usize> = HashMap::new();
    for t in titles.clone() {
        *total.entry(t).or_default() += 1;
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    titles
        .map(|t| {
            let n = seen.entry(t).or_default();
            *n += 1;
            if total[t] > 1 {
                format!("{t} {n}")
            } else {
                t.to_string()
            }
        })
        .collect()
}

/// Compute the pixel width of one tab given its label and UI char width.
/// Geometry: left-pad(12) + type-icon(18) + label + health-gap(8) +
/// close(ucw) + right-pad(12).  The session bar renderer and the app's tab
/// hit-test both call this so widths stay in lockstep.
pub fn tab_width(label: &str, ucw: f32) -> f32 {
    12.0 + 18.0 + label.chars().count() as f32 * ucw + 8.0 + ucw + 12.0
}

/// Icon glyph for a session kind (top tabs + pane headers).
fn kind_icon(kind: &SessionKind) -> &'static str {
    match kind {
        SessionKind::Local => renderer_windows::icons::TERMINAL,
        SessionKind::Ssh { .. } => renderer_windows::icons::GLOBE,
        SessionKind::Agent { .. } => renderer_windows::icons::SYNC,
    }
}

fn kind_color(kind: &SessionKind) -> [f32; 4] {
    match kind {
        SessionKind::Local => COLOR_LOCAL,
        SessionKind::Ssh { .. } => COLOR_SSH,
        SessionKind::Agent { .. } => COLOR_AGENT,
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn generate_commands(s: &ChromeState<'_>) -> Vec<UiCommand> {
    let mut cmds = Vec::with_capacity(512);

    render_session_bar(s, &mut cmds);
    cmds.extend(sidebar::render(s));
    render_pane_headers(s, &mut cmds);
    render_pane_borders(s, &mut cmds);
    render_split_handles(s, &mut cmds);
    render_statusbar(s, &mut cmds);

    cmds
}

// ── Session bar (unified titlebar + tabbar) ────────────────────────────────────

fn render_session_bar(s: &ChromeState<'_>, cmds: &mut Vec<UiCommand>) {
    let sb = s.layout.session_bar;
    let ch = s.cell_h as f32;
    let ucw = s.ui_char_w;

    // Background + bottom separator
    cmds.push(fill(sb, BG_SURFACE));
    cmds.push(bottom_border(sb, BORDER_SUBTLE));

    // ── Caption buttons (Windows 11 spec: full-bleed backplates, top-right,
    // min · max · close with close rightmost; neutral icons at rest, close
    // hover = system red with a white glyph) ──────────────────────────────────
    let zone_x = sb.x + sb.w - CAPTION_ZONE_W;
    let btn_fns: &[(&str, usize)] = &[
        (renderer_windows::icons::MINIMIZE, 0),
        (renderer_windows::icons::MAXIMIZE, 1),
        (renderer_windows::icons::CLOSE, 2),
    ];
    for (glyph, idx) in btn_fns {
        let bx = zone_x + *idx as f32 * CAPTION_BTN_W;
        let is_close = *idx == 2;
        let hovered = hit_test(s, bx, sb.y, CAPTION_BTN_W, sb.h);
        let (btn_bg, glyph_fg) = match (hovered, is_close) {
            (true, true) => (CAPTION_CLOSE_HOVER_BG, CAPTION_CLOSE_HOVER_FG),
            (true, false) => (BG_HOVER, TEXT_PRIMARY),
            (false, _) => (BG_SURFACE, TEXT_MUTED),
        };
        if hovered {
            cmds.push(UiCommand::FillRect {
                x: bx, y: sb.y, w: CAPTION_BTN_W, h: sb.h,
                color: btn_bg,
            });
        }
        // Caption icons sit slightly small; centre the icon cell in the plate.
        cmds.push(UiCommand::DrawIcon {
            x: bx + (CAPTION_BTN_W - 12.0) * 0.5,
            y: sb.y + (sb.h - ch) * 0.5,
            text: glyph.to_string(),
            fg: glyph_fg,
            bg: btn_bg,
            large: false,
        });
    }

    // ── Session tabs (caption buttons live on the right, so tabs start at
    // the left edge with a small pad) ─────────────────────────────────────────
    let tab_start_x = sb.x + super::layout::TAB_PAD_LEFT;
    let tab_text_y  = sb.y + (sb.h - ch) * 0.5;
    let tab_dot_cy  = sb.y + sb.h * 0.5;
    let mut tab_x   = tab_start_x;
    let labels = display_labels(s.sessions.iter().map(|s| s.title.as_str()));

    for (i, session) in s.sessions.iter().enumerate() {
        let is_active  = i == s.active_tab;
        let exit_code  = s.tab_exit_codes.get(i).copied().flatten();

        let label = labels[i].as_str();
        let type_color = kind_color(&session.kind);

        let tab_w  = tab_width(label, ucw);
        let tab_hovered = hit_test(s, tab_x, sb.y, tab_w, sb.h);
        let tab_bg = if is_active { BG_BASE } else { BG_SURFACE };

        if is_active {
            // Full-height BG_BASE fill — connects the tab to the terminal pane,
            // with a 2px type-colour accent edge along the bottom.
            cmds.push(UiCommand::FillRect {
                x: tab_x, y: sb.y, w: tab_w, h: sb.h,
                color: BG_BASE,
            });
            cmds.push(UiCommand::FillRect {
                x: tab_x, y: sb.y + sb.h - 2.0, w: tab_w, h: 2.0,
                color: type_color,
            });
        } else if tab_hovered {
            // Soft rounded hover pill for inactive tabs.
            cmds.push(UiCommand::FillRoundRect {
                x: tab_x + 2.0, y: sb.y + 5.0, w: tab_w - 4.0, h: sb.h - 10.0,
                radius: RADIUS_SM, color: BG_HOVER, bg: BG_SURFACE,
            });
        }

        // Session-type icon.
        let icon_fg = if is_active { type_color } else { TEXT_MUTED };
        cmds.push(UiCommand::DrawIcon {
            x: tab_x + 12.0,
            y: tab_text_y,
            text: kind_icon(&session.kind).to_string(),
            fg: icon_fg,
            bg: tab_bg,
            large: false,
        });

        // Session label (semibold when active).
        let text_fg = if is_active { TEXT_PRIMARY } else { TEXT_MUTED };
        let label_style = if is_active { UiTextStyle::Bold } else { UiTextStyle::Body };
        cmds.push(UiCommand::DrawStyledText {
            x: tab_x + 30.0,
            y: tab_text_y,
            text: label.to_string(),
            fg: text_fg,
            bg: tab_bg,
            style: label_style,
        });

        // Health dot: amber when the session is awaiting input, else grey = no
        // commands, green = last exit 0, red = non-zero.
        let health_col = if s.awaiting.get(i).copied().unwrap_or(false) {
            AMBER
        } else {
            match exit_code {
                None    => TEXT_FAINT,
                Some(0) => GREEN,
                Some(_) => RED,
            }
        };
        let label_w   = label.chars().count() as f32 * ucw;
        let health_cx = tab_x + 30.0 + label_w + 8.0;
        cmds.push(UiCommand::DrawCircle {
            cx: health_cx, cy: tab_dot_cy,
            r: 2.0,
            fg: health_col,
            bg: tab_bg,
        });

        // Close icon — on active tab always, on inactive only when hovered.
        if is_active || tab_hovered {
            let close_fg = if tab_hovered && !is_active { TEXT_MUTED } else { TEXT_FAINT };
            cmds.push(UiCommand::DrawIcon {
                x: tab_x + tab_w - ucw - 12.0,
                y: tab_text_y,
                text: renderer_windows::icons::CLOSE.to_string(),
                fg: close_fg,
                bg: tab_bg,
                large: false,
            });
        }

        tab_x += tab_w + 1.0;
    }

    // "+" new-tab button with hover.
    let plus_hovered = hover_or(s, tab_x + 4.0, sb.y, 28.0, sb.h, BG_SURFACE) == BG_HOVER;
    if plus_hovered {
        cmds.push(UiCommand::FillRoundRect {
            x: tab_x + 4.0, y: sb.y + 5.0, w: 28.0, h: sb.h - 10.0,
            radius: RADIUS_SM, color: BG_HOVER, bg: BG_SURFACE,
        });
    }
    cmds.push(UiCommand::DrawIcon {
        x: tab_x + 4.0 + (28.0 - 12.0) * 0.5,
        y: sb.y + (sb.h - ch) * 0.5,
        text: renderer_windows::icons::ADD.to_string(),
        fg: if plus_hovered { TEXT_PRIMARY } else { TEXT_MUTED },
        bg: BG_SURFACE,
        large: false,
    });

    // Settings gear — right-aligned, left of the caption-button zone.
    let gear_x = sb.x + sb.w - CAPTION_ZONE_W - 36.0;
    let gear_w = 32.0;
    let gear_hovered = hover_or(s, gear_x, sb.y, gear_w, sb.h, BG_SURFACE) == BG_HOVER;
    if gear_hovered {
        cmds.push(UiCommand::FillRoundRect {
            x: gear_x, y: sb.y + 5.0, w: gear_w, h: sb.h - 10.0,
            radius: RADIUS_SM, color: BG_HOVER, bg: BG_SURFACE,
        });
    }
    cmds.push(UiCommand::DrawIcon {
        x: gear_x + (gear_w - 12.0) * 0.5,
        y: sb.y + (sb.h - ch) * 0.5,
        text: renderer_windows::icons::SETTINGS.to_string(),
        fg: if gear_hovered { TEXT_PRIMARY } else { TEXT_MUTED },
        bg: BG_SURFACE,
        large: false,
    });
}

// ── Pane headers ──────────────────────────────────────────────────────────────

fn render_pane_headers(s: &ChromeState<'_>, cmds: &mut Vec<UiCommand>) {
    let ch  = s.cell_h as f32;
    let ucw = s.ui_char_w;

    for &(px, py, pw, _ph, session_idx) in s.pane_rects {
        if let Some(session) = s.sessions.get(session_idx) {
            let is_active = session_idx == s.active_pane_session_idx;
            let hdr = Rect { x: px, y: py, w: pw, h: PANE_HEADER_H };
            let accent = kind_color(&session.kind);

            // Header backplate: rounded top corners (to match the card border),
            // squared bottom so it meets the terminal body cleanly.
            cmds.push(UiCommand::FillRoundRect {
                x: px, y: py, w: pw, h: PANE_HEADER_H, radius: RADIUS_MD,
                color: BG_SURFACE, bg: BG_BASE,
            });
            cmds.push(UiCommand::FillRect {
                x: px, y: py + RADIUS_MD, w: pw, h: PANE_HEADER_H - RADIUS_MD,
                color: BG_SURFACE,
            });
            // Hairline under the header.
            cmds.push(UiCommand::FillRect {
                x: px + RADIUS_MD, y: py + PANE_HEADER_H - 1.0, w: pw - RADIUS_MD * 2.0, h: 1.0,
                color: BORDER_SUBTLE,
            });

            // Session-type icon (neutral).
            let _ = accent;
            let label_y = py + (hdr.h - ch) * 0.5;
            cmds.push(UiCommand::DrawIcon {
                x: px + 12.0, y: label_y,
                text: kind_icon(&session.kind).to_string(),
                fg: TEXT_MUTED, bg: BG_SURFACE, large: false,
            });

            // Single clean label: the working directory (OSC 7) when known,
            // else the session title — in body weight, no oversized title.
            let label_fg = if is_active { TEXT_PRIMARY } else { TEXT_MUTED };
            let label_x = px + 34.0;
            let avail = (pw - (label_x - px) - 72.0).max(0.0);
            let max_chars = (avail / ucw) as usize;
            let label = match session.cwd.as_deref() {
                Some(cwd) if max_chars >= 5 => shorten_path(cwd, max_chars),
                _ => session.title.clone(),
            };
            cmds.push(UiCommand::DrawStyledText {
                x: label_x, y: label_y, text: label,
                fg: label_fg, bg: BG_SURFACE, style: UiTextStyle::Body,
            });

            // Right-aligned pane actions: split · close (icons, hover plates).
            let act_y = py + (hdr.h - ch) * 0.5;
            let close_x = px + pw - 28.0;
            if hover_or(s, close_x - 4.0, py + 4.0, 26.0, PANE_HEADER_H - 8.0, BG_SURFACE) == BG_HOVER {
                cmds.push(UiCommand::FillRoundRect {
                    x: close_x - 4.0, y: py + 5.0, w: 26.0, h: PANE_HEADER_H - 10.0,
                    radius: RADIUS_SM, color: BG_HOVER, bg: BG_SURFACE,
                });
            }
            cmds.push(UiCommand::DrawIcon {
                x: close_x, y: act_y, text: renderer_windows::icons::CLOSE.to_string(),
                fg: TEXT_MUTED, bg: BG_SURFACE, large: false,
            });
            let split_x = px + pw - 54.0;
            if hover_or(s, split_x - 4.0, py + 4.0, 26.0, PANE_HEADER_H - 8.0, BG_SURFACE) == BG_HOVER {
                cmds.push(UiCommand::FillRoundRect {
                    x: split_x - 4.0, y: py + 5.0, w: 26.0, h: PANE_HEADER_H - 10.0,
                    radius: RADIUS_SM, color: BG_HOVER, bg: BG_SURFACE,
                });
            }
            cmds.push(UiCommand::DrawIcon {
                x: split_x, y: act_y, text: renderer_windows::icons::MORE.to_string(),
                fg: TEXT_MUTED, bg: BG_SURFACE, large: false,
            });
        }
    }
}

// ── Pane card borders + notification ring ──────────────────────────────────────

fn render_pane_borders(s: &ChromeState<'_>, cmds: &mut Vec<UiCommand>) {
    for &(px, py, pw, ph, session_idx) in s.pane_rects {
        let is_active = session_idx == s.active_pane_session_idx;
        let awaiting = s.awaiting.get(session_idx).copied().unwrap_or(false);
        let accent = s.sessions.get(session_idx).map(|sn| kind_color(&sn.kind)).unwrap_or(BORDER_DEFAULT);

        // Awaiting-input glow: expanding translucent rings (cmux signature).
        if awaiting {
            for i in 0..RING_GLOW_LAYERS {
                let g = (i + 1) as f32 * 2.0;
                let a = RING_GLOW_ALPHA * (1.0 - i as f32 / RING_GLOW_LAYERS as f32);
                cmds.push(UiCommand::StrokeRoundRect {
                    x: px - g, y: py - g, w: pw + g * 2.0, h: ph + g * 2.0,
                    radius: RADIUS_MD + g, thickness: 2.0,
                    color: [AMBER[0], AMBER[1], AMBER[2], a],
                });
            }
        }

        // Card border: type-colour ring when active (or awaiting), else subtle.
        let (color, thickness) = if awaiting {
            (AMBER, RING_W + 0.5)
        } else if is_active {
            (accent, RING_W)
        } else {
            (BORDER_DEFAULT, 1.0)
        };
        cmds.push(UiCommand::StrokeRoundRect {
            x: px, y: py, w: pw, h: ph, radius: RADIUS_MD, thickness, color,
        });
    }
}

// ── Split handles ─────────────────────────────────────────────────────────────

fn render_split_handles(s: &ChromeState<'_>, cmds: &mut Vec<UiCommand>) {
    // Panes are inset by PANE_GUTTER into floating cards; align the divider grip
    // to that inset height so it sits in the gap between the two cards.
    let y = s.layout.content.y + PANE_GUTTER;
    let h = (s.layout.content.h - PANE_GUTTER * 2.0).max(0.0);
    for &hx in s.split_handles {
        cmds.push(UiCommand::FillRect {
            x: hx + SPLIT_HANDLE_W * 0.5 - 0.5,
            y: y + h * 0.5 - 14.0,
            w: 1.5,
            h: 28.0,
            color: BORDER_STRONG,
        });
    }
}

// ── Command bar ────────────────────────────────────────────────────────────────

fn render_statusbar(s: &ChromeState<'_>, cmds: &mut Vec<UiCommand>) {
    let stb = s.layout.statusbar;
    let ch  = s.cell_h as f32;
    let ucw = s.ui_char_w;

    cmds.push(fill(stb, BG_SURFACE));
    cmds.push(UiCommand::FillRect {
        x: stb.x, y: stb.y, w: stb.w, h: 1.0,
        color: BORDER_DEFAULT,
    });

    let st_y = stb.y + (stb.h - ch) * 0.5;

    // ── Left: shell name + CWD ────────────────────────────────────────────────
    let mut lx = stb.x + 12.0;

    let shell_str = if s.active_shell_name.is_empty() { "terminal" } else { s.active_shell_name };
    let shell_w = shell_str.chars().count() as f32 * ucw;
    cmds.push(ui_text(shell_str, lx, st_y, TEXT_PRIMARY, BG_SURFACE));
    lx += shell_w;

    if let Some(cwd) = s.active_cwd {
        let sep_w = 2.0 * ucw;
        lx += sep_w;
        let max_chars = ((stb.w * 0.45) / ucw) as usize;
        let display_cwd = if cwd.chars().count() > max_chars {
            let skip = cwd.len().saturating_sub(max_chars);
            format!("\u{2026}{}", &cwd[skip..])
        } else {
            cwd.to_string()
        };
        cmds.push(ui_text(&display_cwd, lx, st_y, TEXT_MUTED, BG_SURFACE));
    }

    // ── Right: last-command exit-code indicator ───────────────────────────────
    let exit_code = s.tab_exit_codes.get(s.active_tab).copied().flatten();

    let dot_cx = stb.x + stb.w - 16.0;
    if let Some(code) = exit_code {
        if code != 0 {
            let code_str = format!("{code}");
            let code_w = code_str.chars().count() as f32 * ucw;
            cmds.push(ui_text(&code_str, dot_cx - code_w - 8.0, st_y, RED, BG_SURFACE));
        }
    }

    let dot_col = match exit_code {
        None    => TEXT_FAINT,
        Some(0) => GREEN,
        Some(_) => RED,
    };
    cmds.push(UiCommand::DrawCircle {
        cx: dot_cx,
        cy: stb.y + stb.h * 0.5,
        r: 4.0,
        fg: dot_col,
        bg: BG_SURFACE,
    });
}

// ── Close confirmation dialog ─────────────────────────────────────────────────

pub fn generate_confirm_close_commands(
    window_w: f32,
    window_h: f32,
    busy_count: usize,
    ch: f32,
) -> Vec<UiCommand> {
    let mut cmds = Vec::with_capacity(12);

    cmds.push(UiCommand::FillRect {
        x: 0.0, y: 0.0, w: window_w, h: window_h,
        color: OVERLAY_DIM,
    });

    let panel_w = 420.0_f32.min(window_w - 40.0);
    let panel_h = ch * 3.0 + 44.0;
    let px = (window_w - panel_w) * 0.5;
    let py = (window_h - panel_h) * 0.4;

    cmds.push(shadow(px, py, panel_w, panel_h, 8.0));
    cmds.push(UiCommand::FillRoundRect {
        x: px, y: py, w: panel_w, h: panel_h, radius: RADIUS_LG,
        color: BG_SURFACE,
        bg: BG_BASE,
    });
    cmds.push(UiCommand::FillRect {
        x: px + 8.0, y: py, w: panel_w - 16.0, h: 2.0,
        color: RED,
    });

    cmds.push(UiCommand::DrawUiText {
        x: px + 16.0, y: py + 12.0,
        text: "Close TermForge?".to_string(),
        fg: TEXT_PRIMARY,
        bg: BG_SURFACE,
    });
    let detail = if busy_count > 0 {
        format!(
            "{busy_count} session{} still active.",
            if busy_count == 1 { " is" } else { "s are" }
        )
    } else {
        "Multiple tabs are open.".to_string()
    };
    cmds.push(UiCommand::DrawUiText {
        x: px + 16.0, y: py + 16.0 + ch,
        text: detail,
        fg: TEXT_MUTED,
        bg: BG_SURFACE,
    });
    cmds.push(UiCommand::DrawUiText {
        x: px + 16.0, y: py + 24.0 + ch * 2.0,
        text: "[Enter/Y] quit    [Esc/N] cancel".to_string(),
        fg: TEXT_FAINT,
        bg: BG_SURFACE,
    });

    cmds
}

// ── New-session welcome overlay ───────────────────────────────────────────────

pub fn shadow(x: f32, y: f32, w: f32, h: f32, radius: f32) -> UiCommand {
    UiCommand::DropShadow {
        x, y, w, h, radius,
        offset_x: 0.0, offset_y: 4.0,
        opacity: 0.35,
    }
}

pub fn generate_init_overlay(
    px: f32,
    py: f32,
    pw: f32,
    ph: f32,
    cell_w: f32,
    cell_h: f32,
    ucw: f32,
) -> Vec<UiCommand> {
    let mut cmds = Vec::new();

    let shortcuts: &[(&str, &str)] = &[
        ("ctrl t",   "new tab"),
        ("ctrl p",   "split / close split pane"),
        ("ctrl w",   "close tab"),
        ("ctrl tab", "cycle to next tab"),
        ("ctrl \\",  "toggle sidebar"),
        ("ctrl h",   "SSH manager"),
        ("ctrl a",   "agent launcher"),
        ("ctrl ,",   "settings"),
        ("ctrl 1-9", "jump to tab by number"),
    ];

    // ── Card geometry ─────────────────────────────────────────────────────────
    let badge_inner_w = 8.0 * cell_w;
    let badge_col_w   = badge_inner_w + 16.0;
    let desc_col_w    = 26.0 * ucw;
    let h_pad         = 20.0;
    let v_pad         = 16.0;
    let row_h         = cell_h + 8.0;

    let card_w = (h_pad + badge_col_w + 16.0 + desc_col_w + h_pad)
        .min(pw - 48.0)
        .max(300.0);

    let header_h = cell_h + 6.0 + cell_h + 10.0 + 1.0 + 8.0;
    let card_h   = v_pad + header_h + shortcuts.len() as f32 * row_h + v_pad;

    if ph < 300.0 || card_h > ph - 32.0 {
        return cmds;
    }

    let card_x = px + 24.0;
    let card_y = (py + (ph - card_h) * 0.5).max(py + 8.0).min(py + ph - card_h - 8.0);

    // ── Card background ───────────────────────────────────────────────────────
    cmds.push(UiCommand::FillRoundRect {
        x: card_x, y: card_y, w: card_w, h: card_h, radius: RADIUS_LG,
        color: BG_HOVER,
        bg: BG_BASE,
    });
    cmds.push(UiCommand::FillRect {
        x: card_x, y: card_y + 8.0, w: 3.0, h: card_h - 16.0,
        color: BLUE,
    });

    // ── Headline ──────────────────────────────────────────────────────────────
    let title_y = card_y + v_pad;
    cmds.push(UiCommand::DrawUiText {
        x: card_x + h_pad,
        y: title_y,
        text: "TermForge".to_string(),
        fg: TEXT_PRIMARY,
        bg: BG_HOVER,
    });

    // ── Subheadline ───────────────────────────────────────────────────────────
    let subtitle_y = title_y + cell_h + 6.0;
    cmds.push(UiCommand::DrawUiText {
        x: card_x + h_pad,
        y: subtitle_y,
        text: "Local \u{00B7} SSH \u{00B7} Agent \u{00B7} unified".to_string(),
        fg: TEXT_MUTED,
        bg: BG_HOVER,
    });

    // ── Separator ─────────────────────────────────────────────────────────────
    let sep_y = subtitle_y + cell_h + 10.0;
    cmds.push(UiCommand::FillRect {
        x: card_x + h_pad, y: sep_y,
        w: card_w - h_pad * 2.0, h: 1.0,
        color: BORDER_DEFAULT,
    });

    // ── Shortcut rows ─────────────────────────────────────────────────────────
    let mut row_y = sep_y + 8.0;
    for (key, desc) in shortcuts {
        cmds.push(UiCommand::FillRoundRect {
            radius: RADIUS_SM,
            x: card_x + h_pad,
            y: row_y - 2.0,
            w: badge_col_w,
            h: cell_h + 4.0,
            color: BG_RAISED,
            bg: BG_HOVER,
        });
        cmds.push(UiCommand::DrawText {
            x: card_x + h_pad + 8.0,
            y: row_y,
            text: (*key).to_string(),
            fg: BLUE,
            bg: BG_RAISED,
        });
        cmds.push(UiCommand::DrawUiText {
            x: card_x + h_pad + badge_col_w + 16.0,
            y: row_y,
            text: (*desc).to_string(),
            fg: TEXT_MUTED,
            bg: BG_HOVER,
        });
        row_y += row_h;
    }

    cmds
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn fill(r: Rect, color: [f32; 4]) -> UiCommand {
    UiCommand::FillRect { x: r.x, y: r.y, w: r.w, h: r.h, color }
}

fn bottom_border(r: Rect, color: [f32; 4]) -> UiCommand {
    UiCommand::BottomBorder { x: r.x, y: r.y, w: r.w, h: r.h, color }
}

fn ui_text(t: &str, x: f32, y: f32, fg: [f32; 4], bg: [f32; 4]) -> UiCommand {
    UiCommand::DrawUiText { x, y, text: t.to_string(), fg, bg }
}

fn shorten_path(path: &str, max_chars: usize) -> String {
    let count = path.chars().count();
    let max_chars = max_chars.max(3);
    if count <= max_chars {
        return path.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let skip = count - keep;
    let tail: String = path.chars().skip(skip).collect();
    format!("\u{2026}{tail}")
}

#[cfg(test)]
mod label_tests {
    use super::display_labels;

    #[test]
    fn unique_titles_unchanged() {
        let titles = ["pwsh", "ssh: prod"];
        assert_eq!(
            display_labels(titles.iter().copied()),
            vec!["pwsh".to_string(), "ssh: prod".to_string()]
        );
    }

    #[test]
    fn duplicates_get_numbered() {
        let titles = ["local", "local", "agent", "local"];
        assert_eq!(
            display_labels(titles.iter().copied()),
            vec![
                "local 1".to_string(),
                "local 2".to_string(),
                "agent".to_string(),
                "local 3".to_string()
            ]
        );
    }
}
