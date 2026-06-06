//! Chrome command generation — orchestrates all sub-renderers into a single
//! flat `Vec<UiCommand>` that the compositor draws each frame.

use libterm::mux::session::SessionKind;
use renderer_dx12::ui_renderer::{
    hex, UiCommand,
    COL_BG, COL_BLUE, COL_BORDER, COL_FAINT, COL_GREEN, COL_HOVER,
    COL_MUTED, COL_PANEL, COL_PURPLE, COL_RAISED, COL_RED, COL_TEXT,
    COL_TL_GREEN, COL_TL_RED, COL_TL_YELLOW,
};

use super::layout::{
    ChromeState, Rect, TL_ZONE_W,
    TL_GAP, TL_RADIUS, TL_X0, TL_Y,
};
use super::sidebar;

pub use super::layout::{
    PANE_HEADER_H, SPLIT_HANDLE_W,
};

/// Compute the pixel width of one tab given its label and UI char width.
/// Used by both the renderer and the hit-test so they stay in sync.
pub fn tab_width(label: &str, ucw: f32) -> f32 {
    // left-pad(14) + dot-gap(10) + label + health-dot(12) + close-gap(ucw+16)
    14.0 + 10.0 + label.chars().count() as f32 * ucw + 12.0 + ucw + 16.0
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
    cmds.push(fill(sb, COL_PANEL));
    cmds.push(bottom_border(sb, COL_BORDER));

    // ── Traffic lights ────────────────────────────────────────────────────────
    for (i, fg) in [COL_TL_RED, COL_TL_YELLOW, COL_TL_GREEN].iter().enumerate() {
        cmds.push(UiCommand::DrawCircle {
            cx: TL_X0 + i as f32 * TL_GAP,
            cy: TL_Y,
            r: TL_RADIUS,
            fg: hex(fg),
            bg: hex(COL_PANEL),
        });
    }

    // ── Session tabs (start after the traffic-light zone) ─────────────────────
    let tab_start_x = sb.x + TL_ZONE_W;
    let tab_text_y  = sb.y + (sb.h - ch) * 0.5;
    let tab_dot_cy  = sb.y + sb.h * 0.5;
    let mut tab_x   = tab_start_x;

    for (i, session) in s.sessions.iter().enumerate() {
        let is_active  = i == s.active_tab;
        let exit_code  = s.tab_exit_codes.get(i).copied().flatten();

        let (type_color, label) = match &session.kind {
            SessionKind::Local      => (COL_GREEN,  session.title.as_str()),
            SessionKind::Ssh { .. } => (COL_BLUE,   session.title.as_str()),
            SessionKind::Agent { .. }=> (COL_PURPLE, session.title.as_str()),
        };

        let tab_w  = tab_width(label, ucw);
        let tab_bg = if is_active { COL_BG } else { COL_PANEL };
        let text_fg = if is_active { COL_TEXT } else { COL_FAINT };

        if is_active {
            // Full-height COL_BG fill — connects the tab visually to the terminal pane
            cmds.push(UiCommand::FillRect {
                x: tab_x, y: sb.y, w: tab_w, h: sb.h,
                color: hex(COL_BG),
            });
            // 2px bottom accent in session-type color
            cmds.push(UiCommand::FillRect {
                x: tab_x + 2.0, y: sb.y + sb.h - 2.0, w: tab_w - 4.0, h: 2.0,
                color: hex(type_color),
            });
            // Left/right 1px COL_BORDER separators
            cmds.push(UiCommand::FillRect {
                x: tab_x, y: sb.y + 4.0, w: 1.0, h: sb.h - 4.0,
                color: hex(COL_BORDER),
            });
            cmds.push(UiCommand::FillRect {
                x: tab_x + tab_w, y: sb.y + 4.0, w: 1.0, h: sb.h - 4.0,
                color: hex(COL_BORDER),
            });
        }

        // Session-type dot (larger on active)
        cmds.push(UiCommand::DrawCircle {
            cx: tab_x + 14.0,
            cy: tab_dot_cy,
            r: if is_active { 3.0 } else { 2.5 },
            fg: hex(type_color),
            bg: hex(tab_bg),
        });

        // Session label
        cmds.push(UiCommand::DrawUiText {
            x: tab_x + 24.0,
            y: tab_text_y,
            text: label.to_string(),
            fg: hex(text_fg),
            bg: hex(tab_bg),
        });

        // Health dot: grey = no commands, green = last exit 0, red = non-zero
        let health_col = match exit_code {
            None    => COL_FAINT,
            Some(0) => COL_GREEN,
            Some(_) => COL_RED,
        };
        let label_w     = label.chars().count() as f32 * ucw;
        let health_cx   = tab_x + 24.0 + label_w + 8.0;
        cmds.push(UiCommand::DrawCircle {
            cx: health_cx, cy: tab_dot_cy,
            r: 2.0,
            fg: hex(health_col),
            bg: hex(tab_bg),
        });

        // Close × — only on active tab, right-aligned
        if is_active {
            cmds.push(UiCommand::DrawUiText {
                x: tab_x + tab_w - ucw - 8.0,
                y: tab_text_y,
                text: "\u{00D7}".to_string(),
                fg: hex(COL_FAINT),
                bg: hex(tab_bg),
            });
        }

        tab_x += tab_w + 1.0;
    }

    // "+" new-tab button
    cmds.push(UiCommand::DrawUiText {
        x: tab_x + 6.0,
        y: tab_text_y,
        text: "+".to_string(),
        fg: hex(COL_MUTED),
        bg: hex(COL_PANEL),
    });

    // Gear ⚙ — right-aligned in session bar
    cmds.push(UiCommand::DrawUiText {
        x: sb.x + sb.w - 28.0,
        y: sb.y + (sb.h - ch) * 0.5,
        text: "\u{2699}".to_string(),
        fg: hex(COL_MUTED),
        bg: hex(COL_PANEL),
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

            cmds.push(fill(hdr, COL_PANEL));
            cmds.push(bottom_border(hdr, COL_BORDER));

            let dot_col = match &session.kind {
                SessionKind::Local      => COL_GREEN,
                SessionKind::Ssh { .. } => COL_BLUE,
                SessionKind::Agent { .. }=> COL_PURPLE,
            };

            // Active pane: 2px left accent bar in session-type color
            if is_active {
                cmds.push(UiCommand::FillRect {
                    x: px,
                    y: py + 2.0,
                    w: 2.0,
                    h: PANE_HEADER_H - 4.0,
                    color: hex(dot_col),
                });
            }

            // Dot — nudged right by 2px on active to clear the accent bar
            let dot_x = if is_active { px + 12.0 } else { px + 10.0 };
            cmds.push(UiCommand::DrawCircle {
                cx: dot_x,
                cy: py + hdr.h * 0.5,
                r: 2.5,
                fg: hex(dot_col),
                bg: hex(COL_PANEL),
            });

            // Label — bright on active pane, faint on inactive
            let label_fg = if is_active { COL_TEXT } else { COL_FAINT };
            cmds.push(UiCommand::DrawUiText {
                x: dot_x + 8.0,
                y: py + (hdr.h - ch) * 0.5,
                text: session.title.clone(),
                fg: hex(label_fg),
                bg: hex(COL_PANEL),
            });

            // Right-aligned pane actions: split · close
            let act_x = px + pw - 8.0;
            let act_y = py + (hdr.h - ch) * 0.5;
            cmds.push(UiCommand::DrawUiText {
                x: act_x - ucw,
                y: act_y,
                text: "\u{00D7}".to_string(),
                fg: hex(COL_FAINT),
                bg: hex(COL_PANEL),
            });
            cmds.push(UiCommand::DrawUiText {
                x: act_x - ucw * 3.0,
                y: act_y,
                text: "\u{2B1C}".to_string(),
                fg: hex(COL_FAINT),
                bg: hex(COL_PANEL),
            });
        }
    }
}

// ── Pane inset borders ────────────────────────────────────────────────────────

/// Draws a 1px border around each terminal content area (below its pane header),
/// creating a subtle "sunken surface" effect that separates terminal from chrome.
fn render_pane_borders(s: &ChromeState<'_>, cmds: &mut Vec<UiCommand>) {
    let c = hex("#252525");
    for &(px, py, pw, ph, _) in s.pane_rects {
        let iy = py + PANE_HEADER_H;
        let ih = ph - PANE_HEADER_H;
        // Top
        cmds.push(UiCommand::FillRect { x: px,          y: iy,          w: pw,  h: 1.0, color: c });
        // Bottom
        cmds.push(UiCommand::FillRect { x: px,          y: iy + ih - 1.0, w: pw, h: 1.0, color: c });
        // Left
        cmds.push(UiCommand::FillRect { x: px,          y: iy,          w: 1.0, h: ih,  color: c });
        // Right
        cmds.push(UiCommand::FillRect { x: px + pw - 1.0, y: iy,        w: 1.0, h: ih,  color: c });
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
            color: hex(COL_BORDER),
        });
    }
}

// ── Command bar (was: status bar) ─────────────────────────────────────────────

fn render_statusbar(s: &ChromeState<'_>, cmds: &mut Vec<UiCommand>) {
    let stb = s.layout.statusbar;
    let ch  = s.cell_h as f32;
    let ucw = s.ui_char_w;

    cmds.push(fill(stb, COL_PANEL));
    // Top border
    cmds.push(UiCommand::FillRect {
        x: stb.x, y: stb.y, w: stb.w, h: 1.0,
        color: hex(COL_BORDER),
    });

    let st_y = stb.y + (stb.h - ch) * 0.5;

    // ── Left: shell name + CWD ────────────────────────────────────────────────
    let mut lx = stb.x + 12.0;

    let shell_str = if s.active_shell_name.is_empty() { "terminal" } else { s.active_shell_name };
    let shell_w = shell_str.chars().count() as f32 * ucw;
    cmds.push(ui_text(shell_str, lx, st_y, COL_TEXT, COL_PANEL));
    lx += shell_w;

    if let Some(cwd) = s.active_cwd {
        let sep_w = 2.0 * ucw; // "  " gap
        lx += sep_w;
        // Truncate CWD to at most 45% of bar width, prepend … if cut
        let max_chars = ((stb.w * 0.45) / ucw) as usize;
        let display_cwd = if cwd.chars().count() > max_chars {
            let skip = cwd.len().saturating_sub(max_chars);
            format!("\u{2026}{}", &cwd[skip..])
        } else {
            cwd.to_string()
        };
        cmds.push(ui_text(&display_cwd, lx, st_y, COL_MUTED, COL_PANEL));
    }

    // ── Right: last-command exit-code indicator ───────────────────────────────
    let exit_code = s.tab_exit_codes.get(s.active_tab).copied().flatten();

    // Numeric exit code label (red, shown only for non-zero)
    let dot_cx = stb.x + stb.w - 16.0;
    if let Some(code) = exit_code {
        if code != 0 {
            let code_str = format!("{code}");
            let code_w = code_str.chars().count() as f32 * ucw;
            cmds.push(ui_text(&code_str, dot_cx - code_w - 8.0, st_y, COL_RED, COL_PANEL));
        }
    }

    // Exit-code dot
    let dot_col = match exit_code {
        None    => COL_FAINT,
        Some(0) => COL_GREEN,
        Some(_) => COL_RED,
    };
    cmds.push(UiCommand::DrawCircle {
        cx: dot_cx,
        cy: stb.y + stb.h * 0.5,
        r: 4.0,
        fg: hex(dot_col),
        bg: hex(COL_PANEL),
    });
}

// ── New-session welcome overlay ───────────────────────────────────────────────

/// Rendered on top of a fresh pane before any keypress.
/// Opens with the TermForge product headline, then keyboard shortcuts.
/// Disappears on first keypress or first PTY output byte.
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

    // Extra height for: title + subtitle + separator vs old: title + separator
    let header_h = cell_h + 6.0 + cell_h + 10.0 + 1.0 + 8.0; // title + subtitle + sep
    let card_h   = v_pad + header_h + shortcuts.len() as f32 * row_h + v_pad;

    // Anchor bottom-left of pane, above where the shell prompt typically sits
    let card_x = px + 24.0;
    let card_y = (py + ph - card_h - 16.0).max(py);

    // ── Card background ───────────────────────────────────────────────────────
    cmds.push(UiCommand::FillRoundRect {
        x: card_x, y: card_y, w: card_w, h: card_h,
        color: hex(COL_HOVER),
        bg: hex(COL_BG),
    });
    // Left accent bar — blue brand colour
    cmds.push(UiCommand::FillRect {
        x: card_x, y: card_y + 8.0, w: 3.0, h: card_h - 16.0,
        color: hex(COL_BLUE),
    });

    // ── Headline: "TermForge" ─────────────────────────────────────────────────
    let title_y = card_y + v_pad;
    cmds.push(UiCommand::DrawUiText {
        x: card_x + h_pad,
        y: title_y,
        text: "TermForge".to_string(),
        fg: hex(COL_TEXT),
        bg: hex(COL_HOVER),
    });

    // ── Subheadline ───────────────────────────────────────────────────────────
    let subtitle_y = title_y + cell_h + 6.0;
    cmds.push(UiCommand::DrawUiText {
        x: card_x + h_pad,
        y: subtitle_y,
        text: "Local \u{00B7} SSH \u{00B7} Agent \u{00B7} unified".to_string(),
        fg: hex(COL_MUTED),
        bg: hex(COL_HOVER),
    });

    // ── Separator ─────────────────────────────────────────────────────────────
    let sep_y = subtitle_y + cell_h + 10.0;
    cmds.push(UiCommand::FillRect {
        x: card_x + h_pad, y: sep_y,
        w: card_w - h_pad * 2.0, h: 1.0,
        color: hex(COL_BORDER),
    });

    // ── Shortcut rows ─────────────────────────────────────────────────────────
    let mut row_y = sep_y + 8.0;
    for (key, desc) in shortcuts {
        // Badge background
        cmds.push(UiCommand::FillRoundRect {
            x: card_x + h_pad,
            y: row_y - 2.0,
            w: badge_col_w,
            h: cell_h + 4.0,
            color: hex(COL_RAISED),
            bg: hex(COL_HOVER),
        });
        // Key label — terminal monospace for keyboard feel
        cmds.push(UiCommand::DrawText {
            x: card_x + h_pad + 8.0,
            y: row_y,
            text: (*key).to_string(),
            fg: hex(COL_BLUE),
            bg: hex(COL_RAISED),
        });
        // Description — proportional UI font
        cmds.push(UiCommand::DrawUiText {
            x: card_x + h_pad + badge_col_w + 16.0,
            y: row_y,
            text: (*desc).to_string(),
            fg: hex(COL_MUTED),
            bg: hex(COL_HOVER),
        });
        row_y += row_h;
    }

    cmds
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn fill(r: Rect, color: &str) -> UiCommand {
    UiCommand::FillRect { x: r.x, y: r.y, w: r.w, h: r.h, color: hex(color) }
}

fn bottom_border(r: Rect, color: &str) -> UiCommand {
    UiCommand::BottomBorder { x: r.x, y: r.y, w: r.w, h: r.h, color: hex(color) }
}

fn ui_text(t: &str, x: f32, y: f32, fg: &str, bg: &str) -> UiCommand {
    UiCommand::DrawUiText { x, y, text: t.to_string(), fg: hex(fg), bg: hex(bg) }
}
