//! Single-GPU compositor — all sessions share one DX12 device.
//!
//! Each call to `render_frame` builds a vertex buffer of cell quads for every
//! visible pane, issues one draw call per pane, and calls Present exactly once.

use anyhow::{Context, Result};
use libterm::grid::cell::Color;
use libterm::grid::TerminalGrid;
use libterm::mux::layout::PaneLayout;
use libterm::mux::session::Session;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;

use crate::context::Dx12Context;
use crate::glyph_atlas::GlyphAtlas;
use crate::pipeline::{CellVertex, RenderPipeline};
use crate::ui_renderer::{self, UiCommand};

/// Max cells per pane (generous upper bound — reallocated if exceeded).
const MAX_CELLS_PER_PANE: usize = 300 * 100;
const VERTS_PER_CELL: usize = 6; // 2 triangles

/// One pane's draw resources (vertex buffer on upload heap).
struct PaneBuffer {
    vb: ID3D12Resource,
    capacity: usize,
    /// Vertex count from the last successful upload. Non-zero means the VB
    /// holds valid data even if the session is not dirty this frame.
    last_vertex_count: usize,
    /// Blink state that produced `last_vertex_count`. Forces a rebuild when
    /// the cursor toggles even if no PTY output arrived this frame.
    last_cursor_visible: bool,
}

pub struct Compositor {
    ctx: Dx12Context,
    pub atlas: GlyphAtlas,
    pipeline: RenderPipeline,
    pane_buffers: Vec<PaneBuffer>,
    chrome_buffer: Option<PaneBuffer>,
    /// RGBA (0..1) background fill colour.
    clear_color: [f32; 4],
    /// Cursor blink state — toggles every 500 ms.
    blink_on: bool,
    last_blink: std::time::Instant,
}

impl Compositor {
    /// Build the compositor.  Uploads the glyph atlas synchronously (waits for
    /// GPU idle before returning).  `dpi` is the monitor DPI (96 = 100 %).
    pub fn build(mut ctx: Dx12Context, font_family: &str, font_size_pt: f32, dpi: f32) -> Result<Self> {
        let pipeline = RenderPipeline::new(&ctx.device).context("RenderPipeline")?;

        // Reset the command list and use it for the atlas upload.
        let frame_alloc = unsafe {
            // Use frame 0's allocator for the one-time upload.
            ctx.flush_gpu()?;
            let alloc: ID3D12CommandAllocator = ctx
                .device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .context("upload allocator")?;
            alloc.Reset().ok();
            let list: ID3D12GraphicsCommandList = ctx
                .device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None)
                .context("upload cmd list")?;
            (alloc, list)
        };
        let (_upload_alloc, upload_list) = frame_alloc;

        let (atlas, _upload_buf) = GlyphAtlas::build(
            &ctx.device,
            &upload_list,
            font_family,
            font_size_pt,
            dpi,
        )
        .context("GlyphAtlas::build")?;

        // Create SRV for atlas at slot 0 of the srv_heap.
        atlas.create_srv(&ctx.device, &ctx.srv_heap, 0);

        // Close + execute the upload list, then wait.
        unsafe {
            upload_list.Close().context("upload list Close")?;
            use windows::core::Interface;
            let cmd: ID3D12CommandList = upload_list.cast().unwrap();
            ctx.cmd_queue.ExecuteCommandLists(&[Some(cmd)]);
            ctx.flush_gpu()?;
            // _upload_buf is dropped here; safe because GPU is idle.
        }

        // Background colour: TermForge dark (#0d1117).
        let clear_color = hex_to_linear("#0d1117");

        Ok(Self {
            ctx,
            atlas,
            pipeline,
            pane_buffers: Vec::new(),
            chrome_buffer: None,
            clear_color,
            blink_on: true,
            last_blink: std::time::Instant::now(),
        })
    }

    /// Cell dimensions from the glyph atlas (pixel size of one character).
    pub fn cell_size(&self) -> (u32, u32) {
        (self.atlas.cell_w, self.atlas.cell_h)
    }

    /// Average advance width of a UI-font (Segoe UI) character in pixels.
    /// Used for approximate layout calculations in chrome.rs.
    pub fn ui_char_w(&self) -> f32 {
        self.atlas.measure_ui_text("abcdefghijklmnopqrstuvwxyz") / 26.0
    }

    /// Returns `true` when the cursor blink period has elapsed and the frame
    /// should be marked dirty even if no PTY output arrived.
    pub fn cursor_blink_due(&self) -> bool {
        self.last_blink.elapsed() >= std::time::Duration::from_millis(500)
    }

    /// Render chrome UI commands + all visible sessions.
    /// `chrome_cmds` are drawn first (tab bar, sidebar, status bar).
    /// `pane_rect` is the pixel rect inside which terminal panes are drawn.
    pub fn render_frame(
        &mut self,
        sessions: &[Session],
        layout: &PaneLayout,
        window_w: u32,
        window_h: u32,
    ) -> Result<()> {
        self.render_frame_with_chrome(sessions, layout, &[], window_w, window_h, None)
    }

    /// `content_rect` = pixel area (x,y,w,h) reserved for terminal panes.
    /// If `None`, the full window is used.
    pub fn render_frame_with_chrome(
        &mut self,
        sessions: &[Session],
        layout: &PaneLayout,
        chrome_cmds: &[UiCommand],
        window_w: u32,
        window_h: u32,
        content_rect: Option<(f32, f32, f32, f32)>,
    ) -> Result<()> {
        unsafe {
            let rtv = self.ctx.begin_frame()?;

            let cl = &self.ctx.cmd_list;

            // Bind pipeline state + root signature.
            cl.SetPipelineState(&self.pipeline.pso);
            cl.SetGraphicsRootSignature(&self.pipeline.root_signature);

            // Bind shader-visible descriptor heap (contains glyph atlas SRV).
            let heaps = [Some(self.ctx.srv_heap.clone())];
            cl.SetDescriptorHeaps(&heaps);

            // Root parameter 1 = descriptor table for glyph atlas (slot 0).
            let gpu_base = self.ctx.srv_heap.GetGPUDescriptorHandleForHeapStart();
            cl.SetGraphicsRootDescriptorTable(1, gpu_base);

            // Viewport + scissor.
            cl.RSSetViewports(&[self.ctx.viewport]);
            cl.RSSetScissorRects(&[self.ctx.scissor]);

            // Set render target and clear.
            cl.OMSetRenderTargets(1, Some(&rtv), false, None);
            cl.ClearRenderTargetView(rtv, &self.clear_color, None);

            cl.IASetPrimitiveTopology(
                windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
            );

            // Root parameter 0 = viewport size as root constants.
            let vp = [window_w as f32, window_h as f32];
            cl.SetGraphicsRoot32BitConstants(0, 2, vp.as_ptr() as *const _, 0);

            // Advance blink state once per frame (outside pane loop so all
            // panes see the same cursor visibility this frame).
            if self.last_blink.elapsed() >= std::time::Duration::from_millis(500) {
                self.blink_on = !self.blink_on;
                self.last_blink = std::time::Instant::now();
            }
            let cursor_visible = self.blink_on;

            // Walk the pane layout tree and draw each pane FIRST —
            // chrome paints on top so sidebar/tabbar/statusbar are never
            // overdrawn by terminal cell quads.
            let (cx, cy, cw, ch) =
                content_rect.unwrap_or((0.0, 0.0, window_w as f32, window_h as f32));
            let pane_rects = layout.rects(cx, cy, cw, ch);
            self.pane_buffers.resize_with(pane_rects.len(), || {
                allocate_vertex_buffer(&self.ctx.device, MAX_CELLS_PER_PANE * VERTS_PER_CELL)
                    .expect("allocate vertex buffer")
            });

            // Only the active (first) session gets a blinking cursor.
            // In split mode the inactive pane has no cursor at all so it does
            // not distract from the focused pane.
            let active_session_id = sessions.first().map(|s| s.id);

            for (pane_idx, (session_id, px, py, pw, ph)) in pane_rects.iter().enumerate() {
                let Some(session) = sessions.iter().find(|s| s.id == *session_id) else {
                    continue;
                };

                // Cursor blink only in the active pane.
                let pane_cursor = cursor_visible && (Some(session.id) == active_session_id);

                let buf = &mut self.pane_buffers[pane_idx];

                // Re-upload vertices when the grid has new content OR when this
                // pane's cursor state changed.
                if session.is_dirty() || buf.last_cursor_visible != pane_cursor {
                    let vertices = build_cell_quads(
                        &session.grid,
                        &self.atlas,
                        *px,
                        *py,
                        *pw,
                        *ph,
                        pane_cursor,
                    );
                    if !vertices.is_empty() {
                        if vertices.len() > buf.capacity {
                            *buf =
                                allocate_vertex_buffer(&self.ctx.device, vertices.len() * 2)?;
                        }
                        let mut ptr = std::ptr::null_mut();
                        buf.vb
                            .Map(
                                0,
                                Some(&D3D12_RANGE { Begin: 0, End: 0 }),
                                Some(&mut ptr),
                            )
                            .context("Map vertex buffer")?;
                        std::ptr::copy_nonoverlapping(
                            vertices.as_ptr(),
                            ptr as *mut CellVertex,
                            vertices.len(),
                        );
                        buf.vb.Unmap(0, None);
                        buf.last_vertex_count = vertices.len();
                    } else {
                        buf.last_vertex_count = 0;
                    }
                    buf.last_cursor_visible = pane_cursor;
                }

                if buf.last_vertex_count == 0 {
                    continue;
                }

                let byte_size = buf.last_vertex_count * std::mem::size_of::<CellVertex>();
                let vb_addr = buf.vb.GetGPUVirtualAddress();
                let vbv = D3D12_VERTEX_BUFFER_VIEW {
                    BufferLocation: vb_addr,
                    SizeInBytes: byte_size as u32,
                    StrideInBytes: std::mem::size_of::<CellVertex>() as u32,
                };
                cl.IASetVertexBuffers(0, Some(&[vbv]));
                cl.DrawInstanced(buf.last_vertex_count as u32, 1, 0, 0);
            }

            // ── Chrome draw call (tab bar, sidebar, status bar) ───────────
            // Drawn AFTER panes so chrome is always on top.
            if !chrome_cmds.is_empty() {
                let chrome_verts = ui_renderer::build_quads(chrome_cmds, &self.atlas);
                if !chrome_verts.is_empty() {
                    let needed = chrome_verts.len();
                    if self
                        .chrome_buffer
                        .as_ref()
                        .is_none_or(|b| b.capacity < needed)
                    {
                        self.chrome_buffer =
                            Some(allocate_vertex_buffer(&self.ctx.device, needed * 2)?);
                    }
                    let buf = self.chrome_buffer.as_mut().unwrap();
                    let byte_size = chrome_verts.len() * std::mem::size_of::<CellVertex>();
                    let mut ptr = std::ptr::null_mut();
                    buf.vb
                        .Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut ptr))
                        .context("Map chrome buffer")?;
                    std::ptr::copy_nonoverlapping(
                        chrome_verts.as_ptr(),
                        ptr as *mut CellVertex,
                        chrome_verts.len(),
                    );
                    buf.vb.Unmap(0, None);

                    let vbv = D3D12_VERTEX_BUFFER_VIEW {
                        BufferLocation: buf.vb.GetGPUVirtualAddress(),
                        SizeInBytes: byte_size as u32,
                        StrideInBytes: std::mem::size_of::<CellVertex>() as u32,
                    };
                    cl.IASetVertexBuffers(0, Some(&[vbv]));
                    cl.DrawInstanced(chrome_verts.len() as u32, 1, 0, 0);
                }
            }

            self.ctx.end_frame()?;

            // Clear dirty flags.
            for session in sessions.iter() {
                session.clear_dirty();
            }

            Ok(())
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.ctx.resize(width, height)
    }

    pub fn device(&self) -> &ID3D12Device {
        &self.ctx.device
    }

    /// Rebuild the glyph atlas at a new DPI (e.g. after a `WM_DPICHANGED`
    /// event).  Waits for GPU idle, replaces the atlas, and re-creates the
    /// SRV so subsequent frames use the new rasterised glyphs.
    ///
    /// All pane vertex buffers are invalidated by clearing their
    /// `last_vertex_count` so the next frame rebuilds every cell quad at the
    /// new cell dimensions.
    pub fn rebuild_atlas(&mut self, font_family: &str, font_size_pt: f32, dpi: f32) -> Result<()> {
        unsafe {
            self.ctx.flush_gpu()?;
            let alloc: windows::Win32::Graphics::Direct3D12::ID3D12CommandAllocator = self
                .ctx
                .device
                .CreateCommandAllocator(
                    windows::Win32::Graphics::Direct3D12::D3D12_COMMAND_LIST_TYPE_DIRECT,
                )
                .context("rebuild_atlas: CreateCommandAllocator")?;
            alloc.Reset().ok();
            let list: windows::Win32::Graphics::Direct3D12::ID3D12GraphicsCommandList = self
                .ctx
                .device
                .CreateCommandList(
                    0,
                    windows::Win32::Graphics::Direct3D12::D3D12_COMMAND_LIST_TYPE_DIRECT,
                    &alloc,
                    None,
                )
                .context("rebuild_atlas: CreateCommandList")?;

            let (new_atlas, _upload_buf) =
                GlyphAtlas::build(&self.ctx.device, &list, font_family, font_size_pt, dpi)
                    .context("rebuild_atlas: GlyphAtlas::build")?;

            list.Close().context("rebuild_atlas: Close")?;
            use windows::core::Interface;
            let cmd: windows::Win32::Graphics::Direct3D12::ID3D12CommandList =
                list.cast().unwrap();
            self.ctx.cmd_queue.ExecuteCommandLists(&[Some(cmd)]);
            self.ctx.flush_gpu()?;
            // _upload_buf dropped here; GPU is idle.

            new_atlas.create_srv(&self.ctx.device, &self.ctx.srv_heap, 0);
            self.atlas = new_atlas;

            // Invalidate all pane vertex caches so the next frame rebuilds
            // with the new cell dimensions.
            for buf in &mut self.pane_buffers {
                buf.last_vertex_count = 0;
                buf.last_cursor_visible = false;
            }
            Ok(())
        }
    }
}

/// Build a flat list of `CellVertex` quads (6 verts per cell) for the visible grid.
/// When `cursor_visible` is true an extra filled-block quad is appended at the
/// current cursor position, drawn in the cell's resolved foreground colour so it
/// always contrasts with the background regardless of theme.
fn build_cell_quads(
    grid: &TerminalGrid,
    atlas: &GlyphAtlas,
    pane_x: f32,
    pane_y: f32,
    pane_w: f32,
    pane_h: f32,
    cursor_visible: bool,
) -> Vec<CellVertex> {
    let cols = grid.cols() as usize;
    let rows = grid.rows() as usize;

    if cols == 0 || rows == 0 {
        return Vec::new();
    }

    // Use the atlas's exact integer cell dimensions rather than dividing the
    // pane width by column count.  Division introduces a fractional cw that
    // accumulates sub-pixel drift across columns — visibly stretching glyphs
    // at column ~40+.  Atlas dimensions are already ceil()'d to integer pixels.
    let cw = atlas.cell_w as f32;
    let ch = atlas.cell_h as f32;

    // Clamp to cells that actually fit in the pane — correct to clip rather
    // than stretch into a partial column.
    let visible_cols = ((pane_w / cw) as usize).min(cols);
    let visible_rows = ((pane_h / ch) as usize).min(rows);

    // +1 slot for the cursor quad.
    let mut verts = Vec::with_capacity((visible_cols * visible_rows + 1) * VERTS_PER_CELL);

    for row in 0..visible_rows {
        for col in 0..visible_cols {
            let cell = match grid.cell(col as u16, row as u16) {
                Some(c) => c,
                None => continue,
            };

            // Pixel-snap the top-left corner to prevent sub-pixel accumulation
            // across columns. Each cell starts at an exact integer boundary.
            let x0 = (pane_x + col as f32 * cw).round();
            let y0 = (pane_y + row as f32 * ch).round();
            let x1 = x0 + cw;
            let y1 = y0 + ch;

            let uv = atlas.uv_for_char(cell.ch);
            let (mut fg, mut bg) = (fg_to_linear(&cell.fg), bg_to_linear(&cell.bg));
            if cell.attrs.inverse {
                std::mem::swap(&mut fg, &mut bg);
            }

            // Quad as two CCW triangles: TL, TR, BL, TR, BR, BL
            let tl = CellVertex { pos: [x0, y0], uv: [uv.u0, uv.v0], fg, bg };
            let tr = CellVertex { pos: [x1, y0], uv: [uv.u1, uv.v0], fg, bg };
            let bl = CellVertex { pos: [x0, y1], uv: [uv.u0, uv.v1], fg, bg };
            let br = CellVertex { pos: [x1, y1], uv: [uv.u1, uv.v1], fg, bg };

            verts.extend_from_slice(&[tl, tr, bl, tr, br, bl]);
        }
    }

    // Cursor: one solid block quad drawn on top of the cell.
    // Uses the space glyph (atlas alpha ≈ 0) so the output colour = bg channel.
    // Fills with the cell's resolved fg colour, which always contrasts with bg.
    // Suppressed when the application has hidden the cursor via ?25l.
    if cursor_visible && grid.is_cursor_visible() {
        let (cc, cr) = grid.cursor();
        let cc = cc as usize;
        let cr = cr as usize;
        if cc < visible_cols && cr < visible_rows {
            let x0 = (pane_x + cc as f32 * cw).round();
            let y0 = (pane_y + cr as f32 * ch).round();
            let x1 = x0 + cw;
            let y1 = y0 + ch;
            let cursor_fill = grid
                .cell(cc as u16, cr as u16)
                .map(|c| fg_to_linear(&c.fg))
                .unwrap_or_else(|| fg_to_linear(&Color::Default));
            let uv = atlas.uv_for_char(' ');
            let zero_fg = [0.0f32; 4];
            let tl = CellVertex { pos: [x0, y0], uv: [uv.u0, uv.v0], fg: zero_fg, bg: cursor_fill };
            let tr = CellVertex { pos: [x1, y0], uv: [uv.u1, uv.v0], fg: zero_fg, bg: cursor_fill };
            let bl = CellVertex { pos: [x0, y1], uv: [uv.u0, uv.v1], fg: zero_fg, bg: cursor_fill };
            let br = CellVertex { pos: [x1, y1], uv: [uv.u1, uv.v1], fg: zero_fg, bg: cursor_fill };
            verts.extend_from_slice(&[tl, tr, bl, tr, br, bl]);
        }
    }

    verts
}

fn allocate_vertex_buffer(device: &ID3D12Device, vertex_count: usize) -> Result<PaneBuffer> {
    let byte_size = (vertex_count * std::mem::size_of::<CellVertex>()) as u64;
    let vb: ID3D12Resource = unsafe {
        let mut vb_opt: Option<ID3D12Resource> = None;
        device
            .CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_UPLOAD,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                    Width: byte_size,
                    Height: 1,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                    Format: DXGI_FORMAT_UNKNOWN,
                    ..Default::default()
                },
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut vb_opt,
            )
            .context("CreateCommittedResource (vertex buffer)")?;
        vb_opt.unwrap()
    };
    Ok(PaneBuffer {
        vb,
        capacity: vertex_count,
        last_vertex_count: 0,
        last_cursor_visible: false,
    })
}

/// Map a foreground `Color` to a linear-space RGBA float4.
/// `Color::Default` → terminal default foreground (#e6edf3, light text).
fn fg_to_linear(color: &Color) -> [f32; 4] {
    match color {
        Color::Default => hex_to_linear("#e6edf3"),
        other => color_to_linear(other),
    }
}

/// Map a background `Color` to a linear-space RGBA float4.
/// `Color::Default` → terminal default background (#0d1117, dark), so empty
/// cells blend into the pane background instead of filling it with light.
fn bg_to_linear(color: &Color) -> [f32; 4] {
    match color {
        Color::Default => hex_to_linear("#0d1117"),
        other => color_to_linear(other),
    }
}

/// Map a concrete (non-default) `Color` to a linear-space RGBA float4.
fn color_to_linear(color: &Color) -> [f32; 4] {
    match color {
        Color::Default => hex_to_linear("#0d1117"), // unreachable; callers handle Default
        Color::Rgb(r, g, b) => [
            srgb_to_linear(*r as f32 / 255.0),
            srgb_to_linear(*g as f32 / 255.0),
            srgb_to_linear(*b as f32 / 255.0),
            1.0,
        ],
        Color::Indexed(i) => {
            let [r, g, b] = ANSI_256[*i as usize];
            [
                srgb_to_linear(r as f32 / 255.0),
                srgb_to_linear(g as f32 / 255.0),
                srgb_to_linear(b as f32 / 255.0),
                1.0,
            ]
        }
    }
}

/// Parse a "#rrggbb" hex colour and return linear-space RGBA.
fn hex_to_linear(hex: &str) -> [f32; 4] {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    [
        srgb_to_linear(r as f32 / 255.0),
        srgb_to_linear(g as f32 / 255.0),
        srgb_to_linear(b as f32 / 255.0),
        1.0,
    ]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Standard ANSI 256-colour palette (first 16 = system colours).
static ANSI_256: [[u8; 3]; 256] = {
    let mut t = [[0u8; 3]; 256];
    // Indices 0-15: XTerm / VTE standard palette (matches Windows Terminal,
    // VS Code, and iTerm2).  The previous CGA values (170, 85) produced
    // washed-out output for cargo/git/ls --color.
    // Standard (0-7):
    t[0]  = [0,   0,   0  ]; // Black
    t[1]  = [128, 0,   0  ]; // Dark Red
    t[2]  = [0,   128, 0  ]; // Dark Green
    t[3]  = [128, 128, 0  ]; // Dark Yellow (Olive)
    t[4]  = [0,   0,   128]; // Dark Blue
    t[5]  = [128, 0,   128]; // Dark Magenta
    t[6]  = [0,   128, 128]; // Dark Cyan
    t[7]  = [192, 192, 192]; // Light Gray
    // Bright (8-15):
    t[8]  = [128, 128, 128]; // Dark Gray
    t[9]  = [255, 0,   0  ]; // Bright Red
    t[10] = [0,   255, 0  ]; // Bright Green
    t[11] = [255, 255, 0  ]; // Bright Yellow
    t[12] = [0,   0,   255]; // Bright Blue
    t[13] = [255, 0,   255]; // Bright Magenta
    t[14] = [0,   255, 255]; // Bright Cyan
    t[15] = [255, 255, 255]; // White
    // 16-231: 6×6×6 colour cube
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
                i += 1;
                b += 1;
            }
            g += 1;
        }
        r += 1;
    }
    // 232-255: greyscale
    let mut k = 0usize;
    while k < 24 {
        t[232 + k] = [8 + k as u8 * 10; 3];
        k += 1;
    }
    t
};
