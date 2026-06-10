//! Block overlay rendering: floating command-result cards rendered on top of
//! each terminal pane, enclosed in solid bounding-box containers with status-
//! coloured borders, and the RTree used for click hit-testing against them.

use libterm::block::store::{BlockId, BlockKind, BlockStatus as BS, CommandBlock};
use libterm::vt::sequences::OscNotification;
use renderer_windows::ui_renderer::UiCommand;
use renderer_windows::tokens::*;
use super::chrome;
use rstar::{RTree, RTreeObject, AABB};

#[derive(Debug, Clone)]
pub struct BlockHitTarget {
    pub block_id: BlockId,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RTreeObject for BlockHitTarget {
    type Envelope = AABB<[f32; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.x, self.y], [self.x + self.w, self.y + self.h])
    }
}

pub fn build_block_rtree(targets: Vec<BlockHitTarget>) -> RTree<BlockHitTarget> {
    RTree::bulk_load(targets)
}

// ── Overlay renderer ──────────────────────────────────────────────────────────

const MAX_VISIBLE_BLOCKS: usize = 3;

pub fn generate_block_overlays(
    blocks: &[CommandBlock],
    pane_x: f32,
    pane_y: f32,
    pane_w: f32,
    cell_w: f32,
    cell_h: f32,
    grid_rows: u16,
) -> (Vec<UiCommand>, Vec<BlockHitTarget>) {
    let mut cmds = Vec::new();
    let mut hits = Vec::new();
    if blocks.is_empty() {
        return (cmds, hits);
    }

    let block_header_h = cell_h + 6.0;
    let margin_v = 4.0;
    let inner_x = pane_x + 8.0;
    let inner_w = pane_w - 16.0;
    let mut y = pane_y + (grid_rows as f32 - 2.0) * cell_h;
    let mut visible: usize = 0;

    for block in blocks.iter().rev() {
        if y < pane_y {
            break;
        }

        let (header_bg, border_col, prompt_col, badge_text, badge_fg) =
            block_colors(block);

        let output_lines = output_line_count(&block.output);
        let output_h = if output_lines > 0 { output_lines as f32 * cell_h + 4.0 } else { 0.0 };
        let block_total_h = block_header_h + margin_v * 2.0 + output_h;
        let block_y = y - block_total_h;
        if block_y < pane_y {
            break;
        }

        visible += 1;
        if visible > MAX_VISIBLE_BLOCKS {
            let remaining = blocks.len() - visible + 1;
            let label = format!("\u{2191} +{remaining} more");
            let label_h = cell_h;
            let label_y = block_y;
            cmds.push(UiCommand::FillRoundRect {
                x: inner_x,
                y: label_y,
                w: inner_w,
                h: label_h + 4.0,
                color: BG_SURFACE,
                bg: BG_BASE,
            });
            cmds.push(UiCommand::DrawUiText {
                x: inner_x + 8.0,
                y: label_y + 2.0,
                text: label,
                fg: TEXT_FAINT,
                bg: BG_SURFACE,
            });
            break;
        }

        hits.push(BlockHitTarget {
            block_id: block.id,
            x: inner_x,
            y: block_y,
            w: inner_w,
            h: block_total_h,
        });

        // Drop shadow behind block container
        cmds.push(chrome::shadow(inner_x, block_y, inner_w, block_total_h, 4.0));

        // ── Bounding box container ─────────────────────────────────────────
        // Full block background: bg-surface (#161b22) under the entire card.
        cmds.push(UiCommand::FillRoundRect {
            x: inner_x,
            y: block_y,
            w: inner_w,
            h: block_total_h,
            color: BG_SURFACE,
            bg: BG_BASE,
        });

        // Outer 0.5px border in the state colour (drawn as a thin rect edge)
        // Top edge
        cmds.push(UiCommand::FillRect {
            x: inner_x, y: block_y,
            w: inner_w, h: 1.0,
            color: border_col,
        });
        // Bottom edge
        cmds.push(UiCommand::FillRect {
            x: inner_x, y: block_y + block_total_h - 1.0,
            w: inner_w, h: 1.0,
            color: border_col,
        });
        // Left edge
        cmds.push(UiCommand::FillRect {
            x: inner_x, y: block_y,
            w: 1.0, h: block_total_h,
            color: border_col,
        });
        // Right edge
        cmds.push(UiCommand::FillRect {
            x: inner_x + inner_w - 1.0, y: block_y,
            w: 1.0, h: block_total_h,
            color: border_col,
        });

        // ── Header row inside the bounding box ──────────────────────────────
        cmds.push(UiCommand::FillRect {
            x: inner_x + 1.0,
            y: block_y + 1.0,
            w: inner_w - 2.0,
            h: block_header_h - 1.0,
            color: header_bg,
        });

        // Left accent bar (full block height, inside border)
        cmds.push(UiCommand::FillRect {
            x: inner_x + 1.0,
            y: block_y + 1.0,
            w: 3.0,
            h: block_total_h - 2.0,
            color: border_col,
        });

        // Prompt symbol + command
        let text_y = block_y + (block_header_h - cell_h) * 0.5;
        let max_cmd_chars = ((inner_w - 100.0) / cell_w) as usize;
        let cmd_display = truncate(&block.command, max_cmd_chars);

        cmds.push(UiCommand::DrawText {
            x: inner_x + 8.0,
            y: text_y,
            text: "\u{276F}".to_string(),
            fg: prompt_col,
            bg: header_bg,
        });
        cmds.push(UiCommand::DrawUiText {
            x: inner_x + 8.0 + cell_w * 1.5,
            y: text_y,
            text: cmd_display,
            fg: TEXT_PRIMARY,
            bg: header_bg,
        });

        // Exit badge
        let badge_w = badge_text.len() as f32 * cell_w * 0.7 + 10.0;
        let badge_rx = inner_x + inner_w - 8.0 - badge_w;
        let badge_ry = block_y + (block_header_h - cell_h - 4.0) * 0.5;
        cmds.push(UiCommand::FillRoundRect {
            x: badge_rx,
            y: badge_ry,
            w: badge_w,
            h: cell_h + 4.0,
            color: header_bg,
            bg: header_bg,
        });
        cmds.push(UiCommand::DrawUiText {
            x: badge_rx + 5.0,
            y: badge_ry + 2.0,
            text: badge_text.to_string(),
            fg: badge_fg,
            bg: header_bg,
        });

        // Duration
        if let Some(ms) = block.duration_ms() {
            let dur = format_duration(ms);
            let dur_w = dur.chars().count() as f32 * cell_w * 0.7;
            cmds.push(UiCommand::DrawUiText {
                x: badge_rx - dur_w - 8.0,
                y: text_y,
                text: dur,
                fg: TEXT_FAINT,
                bg: header_bg,
            });
        }

        // Notification text
        if let Some(OscNotification::Notify(msg)) = &block.notification {
            let max_msg = ((inner_w - 200.0) / cell_w) as usize;
            let display: String = msg.chars().take(max_msg.max(1)).collect();
            cmds.push(UiCommand::DrawUiText {
                x: inner_x + 8.0 + cell_w * 3.0,
                y: text_y,
                text: display,
                fg: AMBER,
                bg: header_bg,
            });
        }

        // Output body
        if output_lines > 0 {
            let body_y = block_y + block_header_h;
            cmds.push(UiCommand::FillRect {
                x: inner_x + 3.0,
                y: body_y,
                w: inner_w - 3.0,
                h: output_h,
                color: BG_SURFACE,
            });
            cmds.push(UiCommand::FillRect {
                x: inner_x + 3.0,
                y: body_y,
                w: inner_w - 3.0,
                h: 1.0,
                color: border_col,
            });
            if is_likely_binary(&block.output) {
                cmds.push(UiCommand::DrawUiText {
                    x: inner_x + 12.0,
                    y: body_y + 4.0,
                    text: "[binary output]".to_string(),
                    fg: TEXT_FAINT,
                    bg: BG_SURFACE,
                });
            } else {
                let output_str = String::from_utf8_lossy(&block.output);
                let mut line_y = body_y + 3.0;
                for line in output_str.lines().take(output_lines) {
                    let display: String = line.chars().take(max_cmd_chars + 10).collect();
                    let line_col = if line.contains("FAIL")
                        || line.to_ascii_lowercase().contains("error")
                    {
                        RED
                    } else {
                        TEXT_MUTED
                    };
                    cmds.push(UiCommand::DrawUiText {
                        x: inner_x + 12.0,
                        y: line_y,
                        text: display,
                        fg: line_col,
                        bg: BG_SURFACE,
                    });
                    line_y += cell_h;
                }
            }
        }

        y = block_y - margin_v;
    }

    (cmds, hits)
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn block_colors(
    block: &CommandBlock,
) -> ([f32; 4], [f32; 4], [f32; 4], &'static str, [f32; 4]) {
    if let Some(notif) = &block.notification {
        return match notif {
            OscNotification::StatusAwaiting => {
                (BLOCK_AWAIT_BG, BLOCK_AWAIT_BD, AMBER, "wait", AMBER)
            }
            OscNotification::StatusSuccess => {
                (BLOCK_SUCCESS_BG, BLOCK_SUCCESS_BD, GREEN, "ok", GREEN)
            }
            OscNotification::StatusError => {
                (BLOCK_ERROR_BG, BLOCK_ERROR_BD, RED, "err", RED)
            }
            OscNotification::Notify(_) => {
                (BLOCK_AWAIT_BG, BLOCK_AWAIT_BD, AMBER, "msg", AMBER)
            }
        };
    }
    match (&block.status, &block.kind) {
        (BS::Success, BlockKind::Agent) => {
            (BLOCK_AGENT_BG, BLOCK_AGENT_BD, PURPLE, "ok", GREEN)
        }
        (BS::Error, BlockKind::Agent) => {
            (BLOCK_AGENT_BG, BLOCK_AGENT_BD, PURPLE, "err", RED)
        }
        (BS::Running, BlockKind::Agent) => {
            (BLOCK_AGENT_BG, BLOCK_AGENT_BD, PURPLE, "run", PURPLE)
        }
        (BS::Success, _) => (BLOCK_SUCCESS_BG, BLOCK_SUCCESS_BD, GREEN, "ok", GREEN),
        (BS::Error, _) => (BLOCK_ERROR_BG, BLOCK_ERROR_BD, RED, "err", RED),
        (BS::Running, _) => (BLOCK_RUNNING_BG, BLOCK_RUNNING_BD, BLUE, "run", BLUE),
        (BS::Cancelled, _) => (BG_SURFACE, BORDER_DEFAULT, TEXT_MUTED, "---", TEXT_MUTED),
    }
}

fn output_line_count(output: &[u8]) -> usize {
    let mut n = 0;
    for &b in output {
        if b == b'\n' {
            n += 1;
            if n > 8 {
                return 8;
            }
        }
    }
    n
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max && max > 3 {
        let cut = s.char_indices().nth(max - 1).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}\u{2026}", &s[..cut])
    } else {
        s.to_string()
    }
}

fn format_duration(ms: u128) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f32 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

fn is_likely_binary(output: &[u8]) -> bool {
    let scan = output.len().min(1024);
    if scan == 0 {
        return false;
    }
    let non_printable = output[..scan]
        .iter()
        .filter(|&&b| b < 0x20 && b != b'\n' && b != b'\t' && b != b'\r')
        .count();
    non_printable > scan / 10
}
