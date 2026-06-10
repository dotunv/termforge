//! Chrome command generation — orchestrates all sub-renderers into a single
//! flat `Vec<UiCommand>` that the compositor draws each frame.

use libterm::mux::session::SessionKind;
use renderer_windows::ui_renderer::UiCommand;
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
pub fn tab_width(label: &str, ucw: f32) -> f32 {
    // left-pad(10) + dot-gap(6) + label + health-dot(6) + close-gap(ucw+8)
    10.0 + 6.0 + label.chars().count() as f32 * ucw + 6.0 + ucw + 8.0
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
    cmds.push(bottom_border(sb, BORDER_DEFAULT));

    // ── Caption buttons (Windows 11 spec: full-bleed backplates, top-right,
    // min · max · close with close rightmost; neutral glyphs at rest, close
    // hover = system red with a white glyph) ──────────────────────────────────
    let zone_x = sb.x + sb.w - CAPTION_ZONE_W;
    let btn_fns: &[(&str, usize)] = &[
        ("\u{2212}", 0), // − minimize
        ("\u{25A1}", 1), // □ maximize
        ("\u{2715}", 2), // ✕ close
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
        // Centre the glyph in the backplate (glyphs are ~1 UI char wide).
        cmds.push(UiCommand::DrawUiText {
            x: bx + (CAPTION_BTN_W - ucw) * 0.5,
            y: sb.y + (sb.h - ch) * 0.5,
            text: glyph.to_string(),
            fg: glyph_fg,
            bg: btn_bg,
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
        let type_color = match &session.kind {
            SessionKind::Local       => COLOR_LOCAL,
            SessionKind::Ssh { .. }  => COLOR_SSH,
            SessionKind::Agent { .. } => COLOR_AGENT,
        };

        let tab_w  = tab_width(label, ucw);
        let tab_bg = if is_active { BG_BASE } else { BG_SURFACE };
        let text_fg = if is_active { TEXT_PRIMARY } else { TEXT_FAINT };

        if is_active {
            // Full-height BG_BASE fill — connects the tab visually to the terminal pane
            cmds.push(UiCommand::FillRect {
                x: tab_x, y: sb.y, w: tab_w, h: sb.h,
                color: BG_BASE,
            });
            // 3px bottom accent gradient (two overlapped rects for subtle fade)
            let accent = type_color;
            cmds.push(UiCommand::FillRect {
                x: tab_x + 2.0, y: sb.y + sb.h - 3.0, w: tab_w - 4.0, h: 3.0,
                color: accent,
            });
            cmds.push(UiCommand::FillRect {
                x: tab_x + 2.0, y: sb.y + sb.h - 1.0, w: tab_w - 4.0, h: 1.0,
                color: [accent[0] * 0.6, accent[1] * 0.6, accent[2] * 0.6, 0.5],
            });
            // Left/right 1px BORDER_DEFAULT separators
            cmds.push(UiCommand::FillRect {
                x: tab_x, y: sb.y + 4.0, w: 1.0, h: sb.h - 4.0,
                color: BORDER_DEFAULT,
            });
            cmds.push(UiCommand::FillRect {
                x: tab_x + tab_w, y: sb.y + 4.0, w: 1.0, h: sb.h - 4.0,
                color: BORDER_DEFAULT,
            });
        } else {
            // Hover background for inactive tabs
            let h_bg = hover_or(s, tab_x, sb.y, tab_w, sb.h, BG_SURFACE);
            if h_bg != BG_SURFACE {
                cmds.push(UiCommand::FillRect {
                    x: tab_x + 1.0, y: sb.y + 4.0, w: tab_w - 2.0, h: sb.h - 8.0,
                    color: BG_HOVER,
                });
            }
        }

        // Session-type dot (larger on active)
        cmds.push(UiCommand::DrawCircle {
            cx: tab_x + 14.0,
            cy: tab_dot_cy,
            r: if is_active { 3.0 } else { 2.5 },
            fg: type_color,
            bg: tab_bg,
        });

        // Session label
        cmds.push(UiCommand::DrawUiText {
            x: tab_x + 24.0,
            y: tab_text_y,
            text: label.to_string(),
            fg: text_fg,
            bg: tab_bg,
        });

        // Health dot: grey = no commands, green = last exit 0, red = non-zero
        let health_col = match exit_code {
            None    => TEXT_FAINT,
            Some(0) => GREEN,
            Some(_) => RED,
        };
        let label_w     = label.chars().count() as f32 * ucw;
        let health_cx   = tab_x + 24.0 + label_w + 8.0;
        cmds.push(UiCommand::DrawCircle {
            cx: health_cx, cy: tab_dot_cy,
            r: 2.0,
            fg: health_col,
            bg: tab_bg,
        });

        // Close × — on active tab always, on inactive tabs only when hovered
        let tab_hovered = hit_test(s, tab_x, sb.y, tab_w, sb.h);
        if is_active || tab_hovered {
            let close_fg = if tab_hovered && !is_active { TEXT_MUTED } else { TEXT_FAINT };
            cmds.push(UiCommand::DrawUiText {
                x: tab_x + tab_w - ucw - 8.0,
                y: tab_text_y,
                text: "\u{00D7}".to_string(),
                fg: close_fg,
                bg: tab_bg,
            });
        }

        // Inactive tab hover: subtle left accent bar
        if tab_hovered && !is_active {
            let accent = type_color;
            cmds.push(UiCommand::FillRect {
                x: tab_x, y: sb.y + 8.0, w: 2.0, h: sb.h - 16.0,
                color: [accent[0] * 0.5, accent[1] * 0.5, accent[2] * 0.5, 0.4],
            });
        }

        tab_x += tab_w + 1.0;
    }

    // "+" new-tab button with hover
    if hover_or(s, tab_x + 4.0, sb.y, 24.0, sb.h, BG_SURFACE) == BG_HOVER {
        cmds.push(UiCommand::FillRect {
            x: tab_x + 4.0, y: sb.y + 4.0, w: 24.0, h: sb.h - 8.0,
            color: BG_HOVER,
        });
    }
    cmds.push(UiCommand::DrawUiText {
        x: tab_x + 4.0 + (24.0 - ucw) * 0.5,
        y: tab_text_y,
        text: "+".to_string(),
        fg: TEXT_MUTED,
        bg: BG_SURFACE,
    });

    // Gear ⚙ — right-aligned, left of the caption-button zone
    let gear_x = sb.x + sb.w - CAPTION_ZONE_W - 32.0;
    let gear_w = 28.0;
    if hover_or(s, gear_x, sb.y, gear_w, sb.h, BG_SURFACE) == BG_HOVER {
        cmds.push(UiCommand::FillRect {
            x: gear_x, y: sb.y + 4.0, w: gear_w, h: sb.h - 8.0,
            color: BG_HOVER,
        });
    }
    cmds.push(UiCommand::DrawUiText {
        x: gear_x + (gear_w - ucw) * 0.5,
        y: sb.y + (sb.h - ch) * 0.5,
        text: "\u{2699}".to_string(),
        fg: TEXT_MUTED,
        bg: BG_SURFACE,
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

            cmds.push(fill(hdr, BG_SURFACE));
            cmds.push(bottom_border(hdr, BORDER_DEFAULT));

            let dot_col = match &session.kind {
                SessionKind::Local      => COLOR_LOCAL,
                SessionKind::Ssh { .. } => COLOR_SSH,
                SessionKind::Agent { .. }=> COLOR_AGENT,
            };

            // Active pane: 3px left accent bar with gradient
            if is_active {
                let ac = dot_col;
                cmds.push(UiCommand::FillRect {
                    x: px,
                    y: py + 2.0,
                    w: 3.0,
                    h: PANE_HEADER_H - 4.0,
                    color: ac,
                });
                cmds.push(UiCommand::FillRect {
                    x: px + 2.0,
                    y: py + 2.0,
                    w: 1.0,
                    h: PANE_HEADER_H - 4.0,
                    color: [ac[0] * 0.5, ac[1] * 0.5, ac[2] * 0.5, 0.4],
                });
            }

            // Dot — nudged right by 2px on active to clear the accent bar
            let dot_x = if is_active { px + 12.0 } else { px + 10.0 };
            cmds.push(UiCommand::DrawCircle {
                cx: dot_x,
                cy: py + hdr.h * 0.5,
                r: 2.5,
                fg: dot_col,
                bg: BG_SURFACE,
            });

            // Primary label: the working directory (what's *happening*) when the
            // shell reports it via OSC 7, falling back to the static session
            // title.  CWD is the thing that changes with the user's work, so it
            // carries more information than the title.
            let label_fg = if is_active { TEXT_PRIMARY } else { TEXT_FAINT };
            let label_x = dot_x + 8.0;
            let label_y = py + (hdr.h - ch) * 0.5;
            match session.cwd.as_deref() {
                Some(cwd) => {
                    cmds.push(UiCommand::DrawUiText {
                        x: label_x,
                        y: label_y,
                        text: format!("{}  ", session.title),
                        fg: TEXT_FAINT,
                        bg: BG_SURFACE,
                    });
                    let title_w = (session.title.chars().count() as f32 + 2.0) * ucw;
                    let avail = (pw - (label_x - px) - title_w - 64.0).max(0.0);
                    let max_chars = (avail / ucw) as usize;
                    if max_chars >= 5 {
                        cmds.push(UiCommand::DrawUiText {
                            x: label_x + title_w,
                            y: label_y,
                            text: shorten_path(cwd, max_chars),
                            fg: label_fg,
                            bg: BG_SURFACE,
                        });
                    }
                }
                None => {
                    cmds.push(UiCommand::DrawUiText {
                        x: label_x,
                        y: label_y,
                        text: session.title.clone(),
                        fg: label_fg,
                        bg: BG_SURFACE,
                    });
                }
            }

            // Right-aligned pane actions: split · close (with hover)
            let act_x = px + pw - 8.0;
            let act_y = py + (hdr.h - ch) * 0.5;
            let act_h = ch;
            // Close button hover
            let close_x = act_x - ucw - 4.0;
            let close_w = ucw + 8.0;
            if hover_or(s, close_x, act_y, close_w, act_h, BG_SURFACE) == BG_HOVER {
                cmds.push(UiCommand::FillRect {
                    x: close_x, y: act_y, w: close_w, h: act_h,
                    color: BG_HOVER,
                });
            }
            cmds.push(UiCommand::DrawUiText {
                x: act_x - ucw,
                y: act_y,
                text: "\u{00D7}".to_string(),
                fg: TEXT_FAINT,
                bg: BG_SURFACE,
            });
            // Split button hover
            let split_x = act_x - ucw * 3.0 - 4.0;
            let split_w = ucw * 2.0 + 8.0;
            if hover_or(s, split_x, act_y, split_w, act_h, BG_SURFACE) == BG_HOVER {
                cmds.push(UiCommand::FillRect {
                    x: split_x, y: act_y, w: split_w, h: act_h,
                    color: BG_HOVER,
                });
            }
            cmds.push(UiCommand::DrawUiText {
                x: act_x - ucw * 3.0,
                y: act_y,
                text: "\u{2B1C}".to_string(),
                fg: TEXT_FAINT,
                bg: BG_SURFACE,
            });
        }
    }
}

// ── Pane inset borders ────────────────────────────────────────────────────────

fn render_pane_borders(s: &ChromeState<'_>, cmds: &mut Vec<UiCommand>) {
    for &(px, py, pw, ph, _) in s.pane_rects {
        let iy = py + PANE_HEADER_H;
        let ih = ph - PANE_HEADER_H;
        cmds.push(UiCommand::FillRect { x: px,          y: iy,          w: pw,  h: 1.0, color: BORDER_SUBTLE });
        cmds.push(UiCommand::FillRect { x: px,          y: iy + ih - 1.0, w: pw, h: 1.0, color: BORDER_SUBTLE });
        cmds.push(UiCommand::FillRect { x: px,          y: iy,          w: 1.0, h: ih,  color: BORDER_SUBTLE });
        cmds.push(UiCommand::FillRect { x: px + pw - 1.0, y: iy,        w: 1.0, h: ih,  color: BORDER_SUBTLE });
    }
}

// ── Split handles ─────────────────────────────────────────────────────────────

fn render_split_handles(s: &ChromeState<'_>, cmds: &mut Vec<UiCommand>) {
    for &hx in s.split_handles {
        cmds.push(UiCommand::FillRect {
            x: hx + SPLIT_HANDLE_W * 0.5 - 0.5,
            y: s.layout.content.y,
            w: 1.0,
            h: s.layout.content.h,
            color: BORDER_DEFAULT,
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
        x: px, y: py, w: panel_w, h: panel_h,
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
        x: card_x, y: card_y, w: card_w, h: card_h,
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
