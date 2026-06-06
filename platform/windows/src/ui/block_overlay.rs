//! Block overlay rendering: floating command-result cards rendered on top of
//! each terminal pane, and the RTree used for click hit-testing against them.

use libterm::block::store::{BlockId, BlockKind, BlockStatus as BS, CommandBlock};
use libterm::vt::sequences::OscNotification;
use renderer_dx12::ui_renderer::{
    hex, UiCommand,
    COL_AMBER, COL_BG, COL_BLOCK_AGENT_BD, COL_BLOCK_AGENT_BG, COL_BLOCK_AWAIT_BD,
    COL_BLOCK_AWAIT_BG, COL_BLOCK_ERROR_BD, COL_BLOCK_ERROR_BG, COL_BLOCK_RUNNING_BD,
    COL_BLOCK_RUNNING_BG, COL_BLOCK_SUCCESS_BD, COL_BLOCK_SUCCESS_BG, COL_BLUE, COL_BORDER,
    COL_FAINT, COL_GREEN, COL_MUTED, COL_PANEL, COL_PURPLE, COL_RED, COL_TEXT,
};
use rstar::{RTree, RTreeObject, AABB};

// ── Hit target ────────────────────────────────────────────────────────────────

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

        hits.push(BlockHitTarget {
            block_id: block.id,
            x: inner_x,
            y: block_y,
            w: inner_w,
            h: block_total_h,
        });

        // Header rounded rect
        cmds.push(UiCommand::FillRoundRect {
            x: inner_x,
            y: block_y,
            w: inner_w,
            h: block_header_h,
            color: hex(header_bg),
            bg: hex(COL_BG),
        });

        // Left accent bar (full block height)
        cmds.push(UiCommand::FillRect {
            x: inner_x,
            y: block_y,
            w: 3.0,
            h: block_total_h,
            color: hex(border_col),
        });

        // Prompt symbol + command
        let text_y = block_y + (block_header_h - cell_h) * 0.5;
        let max_cmd_chars = ((inner_w - 100.0) / cell_w) as usize;
        let cmd_display = truncate(&block.command, max_cmd_chars);

        cmds.push(UiCommand::DrawText {
            x: inner_x + 8.0,
            y: text_y,
            text: "\u{276F}".to_string(),
            fg: hex(prompt_col),
            bg: hex(header_bg),
        });
        cmds.push(UiCommand::DrawUiText {
            x: inner_x + 8.0 + cell_w * 1.5,
            y: text_y,
            text: cmd_display,
            fg: hex(COL_TEXT),
            bg: hex(header_bg),
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
            color: hex(header_bg),
            bg: hex(header_bg),
        });
        cmds.push(UiCommand::DrawUiText {
            x: badge_rx + 5.0,
            y: badge_ry + 2.0,
            text: badge_text.to_string(),
            fg: hex(badge_fg),
            bg: hex(header_bg),
        });

        // Duration
        if let Some(ms) = block.duration_ms() {
            let dur = format_duration(ms);
            let dur_w = dur.chars().count() as f32 * cell_w * 0.7;
            cmds.push(UiCommand::DrawUiText {
                x: badge_rx - dur_w - 8.0,
                y: text_y,
                text: dur,
                fg: hex(COL_FAINT),
                bg: hex(header_bg),
            });
        }

        // Notify message
        if let Some(OscNotification::Notify(msg)) = &block.notification {
            let max_msg = ((inner_w - 200.0) / cell_w) as usize;
            let display: String = msg.chars().take(max_msg.max(1)).collect();
            cmds.push(UiCommand::DrawUiText {
                x: inner_x + 8.0 + cell_w * 3.0,
                y: text_y,
                text: display,
                fg: hex(COL_AMBER),
                bg: hex(header_bg),
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
                color: hex(COL_PANEL),
            });
            cmds.push(UiCommand::FillRect {
                x: inner_x + 3.0,
                y: body_y,
                w: inner_w - 3.0,
                h: 1.0,
                color: hex(border_col),
            });
            let output_str = String::from_utf8_lossy(&block.output);
            let mut line_y = body_y + 3.0;
            for line in output_str.lines().take(output_lines) {
                let display: String = line.chars().take(max_cmd_chars + 10).collect();
                let line_col = if line.contains("FAIL")
                    || line.to_ascii_lowercase().contains("error")
                {
                    COL_RED
                } else {
                    COL_MUTED
                };
                cmds.push(UiCommand::DrawUiText {
                    x: inner_x + 12.0,
                    y: line_y,
                    text: display,
                    fg: hex(line_col),
                    bg: hex(COL_PANEL),
                });
                line_y += cell_h;
            }
        }

        y = block_y - margin_v;
    }

    (cmds, hits)
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn block_colors(
    block: &CommandBlock,
) -> (&'static str, &'static str, &'static str, &'static str, &'static str) {
    if let Some(notif) = &block.notification {
        return match notif {
            OscNotification::StatusAwaiting => {
                (COL_BLOCK_AWAIT_BG, COL_BLOCK_AWAIT_BD, COL_AMBER, "wait", COL_AMBER)
            }
            OscNotification::StatusSuccess => {
                (COL_BLOCK_SUCCESS_BG, COL_BLOCK_SUCCESS_BD, COL_GREEN, "ok", COL_GREEN)
            }
            OscNotification::StatusError => {
                (COL_BLOCK_ERROR_BG, COL_BLOCK_ERROR_BD, COL_RED, "err", COL_RED)
            }
            OscNotification::Notify(_) => {
                (COL_BLOCK_AWAIT_BG, COL_BLOCK_AWAIT_BD, COL_AMBER, "msg", COL_AMBER)
            }
        };
    }
    match (&block.status, &block.kind) {
        (BS::Success, BlockKind::Agent) => {
            (COL_BLOCK_AGENT_BG, COL_BLOCK_AGENT_BD, COL_PURPLE, "ok", COL_GREEN)
        }
        (BS::Error, BlockKind::Agent) => {
            (COL_BLOCK_AGENT_BG, COL_BLOCK_AGENT_BD, COL_PURPLE, "err", COL_RED)
        }
        (BS::Running, BlockKind::Agent) => {
            (COL_BLOCK_AGENT_BG, COL_BLOCK_AGENT_BD, COL_PURPLE, "run", COL_PURPLE)
        }
        (BS::Success, _) => (COL_BLOCK_SUCCESS_BG, COL_BLOCK_SUCCESS_BD, COL_GREEN, "ok", COL_GREEN),
        (BS::Error, _) => (COL_BLOCK_ERROR_BG, COL_BLOCK_ERROR_BD, COL_RED, "err", COL_RED),
        (BS::Running, _) => (COL_BLOCK_RUNNING_BG, COL_BLOCK_RUNNING_BD, COL_BLUE, "run", COL_BLUE),
        (BS::Cancelled, _) => (COL_PANEL, COL_BORDER, COL_MUTED, "---", COL_MUTED),
    }
}

fn output_line_count(output: &[u8]) -> usize {
    if output.is_empty() {
        0
    } else {
        output.iter().filter(|&&b| b == b'\n').count().min(8)
    }
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
