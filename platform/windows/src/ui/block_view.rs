//! Block view — the Warp-style presentation of a session as a scrollable list
//! of command blocks, rendered *instead of* the raw terminal grid when shell
//! integration is active and the shell is not on the alternate screen.
//!
//! Each block is: a divider with the cwd + duration + status icon, the command
//! in bold, then its captured output.  Output is rendered with the real pen
//! colours captured per character (so `ls`/`git`/build colour survives), and
//! carriage-return overwrites are collapsed so progress bars read cleanly.
//! Blocks stack oldest→newest with the newest pinned just above the input bar;
//! the wheel scrolls back through history.

use libterm::block::store::{BlockStatus, CommandBlock};
use renderer_windows::icons;
use renderer_windows::resolve_color;
use renderer_windows::tokens::*;
use renderer_windows::ui_renderer::{UiCommand, UiTextStyle};

/// Cap per-block output lines so a single chatty command can't blow up the
/// per-frame command count; overflow is summarised.
const MAX_OUTPUT_LINES: usize = 240;
/// Only the most recent blocks are laid out (older history is rarely scrolled
/// to and keeps frame cost bounded).
const MAX_BLOCKS: usize = 60;

enum Line {
    Divider { cwd: String, status: BlockStatus, dur: Option<String> },
    Command(String),
    /// Pre-resolved coloured runs for one output line.
    Output(Vec<(String, [f32; 4])>),
    Gap,
}

fn status_icon(s: &BlockStatus) -> &'static str {
    match s {
        BlockStatus::Running => icons::SYNC,
        BlockStatus::Success => icons::CHECK,
        BlockStatus::Error => icons::CANCEL,
        BlockStatus::Cancelled => icons::CANCEL,
    }
}

fn status_color(s: &BlockStatus) -> [f32; 4] {
    match s {
        BlockStatus::Running => TEXT_MUTED,
        BlockStatus::Success => TEXT_MUTED,
        BlockStatus::Error => RED,
        BlockStatus::Cancelled => TEXT_FAINT,
    }
}

/// Maximum scroll-back distance in pixels for the given content/viewport — used
/// by the caller to clamp its stored scroll offset.
pub fn max_scroll(blocks: &[CommandBlock], view_h: f32, cell_h: f32) -> f32 {
    (build_lines(blocks).len() as f32 * cell_h - view_h).max(0.0)
}

fn build_lines(blocks: &[CommandBlock]) -> Vec<Line> {
    let mut lines = Vec::new();
    let start = blocks.len().saturating_sub(MAX_BLOCKS);
    for block in &blocks[start..] {
        // Skip empty prompts (a bare Enter with the prompt suppressed still
        // marks a block) — nothing to show.
        if block.command.trim().is_empty() && block.styled.is_empty() {
            continue;
        }
        let dur = block.duration_ms().map(|ms| {
            if ms >= 1000 { format!("{:.1}s", ms as f32 / 1000.0) } else { format!("{ms}ms") }
        });
        let cwd = block.cwd.as_deref().map(|p| shorten_path(p, 40)).unwrap_or_default();
        lines.push(Line::Divider { cwd, status: block.status.clone(), dur });
        lines.push(Line::Command(block.command.clone()));

        let to_out = |sline: &[libterm::block::store::StyledRun]| -> Vec<(String, [f32; 4])> {
            sline.iter().map(|r| (r.text.clone(), resolve_color(&r.fg, TEXT_PRIMARY))).collect()
        };
        let mut n = 0usize;
        for sline in &block.styled {
            if n >= MAX_OUTPUT_LINES {
                lines.push(Line::Output(vec![("…".to_string(), TEXT_FAINT)]));
                break;
            }
            lines.push(Line::Output(to_out(sline)));
            n += 1;
        }
        if n < MAX_OUTPUT_LINES {
            let cur = block.current_runs();
            if !cur.is_empty() {
                lines.push(Line::Output(to_out(&cur)));
            }
        }
        lines.push(Line::Gap);
    }
    lines
}

/// Render the block view into the body rect `(x, y, w, h)` (already excluding
/// the pane header and any input bar).  `scroll` is pixels scrolled up from the
/// bottom (0 = pinned to the newest block).
#[allow(clippy::too_many_arguments)]
pub fn generate_block_view(
    blocks: &[CommandBlock],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    cell_w: f32,
    cell_h: f32,
    ucw: f32,
    scroll: f32,
) -> Vec<UiCommand> {
    let mut cmds = Vec::new();

    // Opaque backdrop hides the raw grid beneath.
    cmds.push(UiCommand::FillRect { x, y, w, h, color: BG_BASE });

    let lines = build_lines(blocks);
    let content_h = lines.len() as f32 * cell_h;
    let scroll = scroll.clamp(0.0, (content_h - h).max(0.0));
    let base = content_h - h - scroll; // content-space y of the viewport top

    let pad_x = SPACE_4;
    let text_x = x + pad_x;
    let out_x = text_x + cell_w * 2.0;
    let max_chars = (((x + w - pad_x) - out_x) / cell_w).max(0.0) as usize;

    for (i, line) in lines.iter().enumerate() {
        let sy = y + (i as f32 * cell_h - base);
        if sy < y - 0.5 || sy + cell_h > y + h + 0.5 {
            continue; // cheap manual clip
        }
        match line {
            Line::Divider { cwd, status, dur } => {
                let dy = sy + cell_h - 1.0;
                cmds.push(UiCommand::FillRect { x: text_x, y: dy, w: w - pad_x * 2.0, h: 1.0, color: BORDER_SUBTLE });
                if !cwd.is_empty() {
                    cmds.push(UiCommand::DrawStyledText {
                        x: text_x, y: sy, text: cwd.clone(),
                        fg: TEXT_FAINT, bg: BG_BASE, style: UiTextStyle::Caption,
                    });
                }
                let icon_x = x + w - pad_x - 16.0;
                cmds.push(UiCommand::DrawIcon {
                    x: icon_x, y: sy, text: status_icon(status).to_string(),
                    fg: status_color(status), bg: BG_BASE, large: false,
                });
                if let Some(d) = dur {
                    let dw = d.chars().count() as f32 * ucw;
                    cmds.push(UiCommand::DrawStyledText {
                        x: icon_x - dw - 8.0, y: sy, text: d.clone(),
                        fg: TEXT_FAINT, bg: BG_BASE, style: UiTextStyle::Caption,
                    });
                }
            }
            Line::Command(cmd) => {
                cmds.push(UiCommand::DrawText {
                    x: text_x, y: sy, text: "\u{276F}".to_string(),
                    fg: TEXT_MUTED, bg: BG_BASE,
                });
                let shown = truncate(cmd, max_chars);
                cmds.push(UiCommand::DrawStyledText {
                    x: out_x, y: sy, text: shown,
                    fg: TEXT_PRIMARY, bg: BG_BASE, style: UiTextStyle::Bold,
                });
            }
            Line::Output(runs) => {
                let mut cx = out_x;
                let mut col = 0usize;
                for (text, color) in runs {
                    if col >= max_chars {
                        break;
                    }
                    let avail = max_chars - col;
                    let shown = if text.chars().count() > avail {
                        truncate(text, avail)
                    } else {
                        text.clone()
                    };
                    let n = shown.chars().count();
                    cmds.push(UiCommand::DrawText { x: cx, y: sy, text: shown, fg: *color, bg: BG_BASE });
                    cx += n as f32 * cell_w;
                    col += n;
                }
            }
            Line::Gap => {}
        }
    }

    if scroll > 1.0 {
        cmds.push(UiCommand::DrawStyledText {
            x: x + w - pad_x - 60.0, y: y + 4.0, text: "\u{25B2} history".to_string(),
            fg: TEXT_FAINT, bg: BG_BASE, style: UiTextStyle::Caption,
        });
    }

    cmds
}

fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() > max {
        let cut = s.char_indices().nth(max.saturating_sub(1)).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}\u{2026}", &s[..cut])
    } else {
        s.to_string()
    }
}

/// Tail-truncate a path to `max` chars with a leading ellipsis.
fn shorten_path(path: &str, max: usize) -> String {
    let count = path.chars().count();
    if count <= max {
        return path.to_string();
    }
    let keep = max.saturating_sub(1);
    let tail: String = path.chars().skip(count - keep).collect();
    format!("\u{2026}{tail}")
}
