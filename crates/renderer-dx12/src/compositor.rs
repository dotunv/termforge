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
    capacity: usize, // in vertices
}

pub struct Compositor {
    ctx: Dx12Context,
    pub atlas: GlyphAtlas,
    pipeline: RenderPipeline,
    pane_buffers: Vec<PaneBuffer>,
    chrome_buffer: Option<PaneBuffer>,
    /// RGBA (0..1) background fill colour.
    clear_color: [f32; 4],
}

impl Compositor {
    /// Build the compositor.  Uploads the glyph atlas synchronously (waits for
    /// GPU idle before returning).
    pub fn build(
        mut ctx: Dx12Context,
        font_family: &str,
        font_size_pt: f32,
    ) -> Result<Self> {
        let pipeline = RenderPipeline::new(&ctx.device).context("RenderPipeline")?;

        // Reset the command list and use it for the atlas upload.
        let frame_alloc = unsafe {
            // Use frame 0's allocator for the one-time upload.
            ctx.flush_gpu()?;
            let alloc: ID3D12CommandAllocator = ctx.device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .context("upload allocator")?;
            alloc.Reset().ok();
            let list: ID3D12GraphicsCommandList = ctx.device
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
            96.0, // assume 96 DPI for Phase 2
        ).context("GlyphAtlas::build")?;

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
        })
    }

    /// Cell dimensions from the glyph atlas (pixel size of one character).
    pub fn cell_size(&self) -> (u32, u32) {
        (self.atlas.cell_w, self.atlas.cell_h)
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

            cl.IASetPrimitiveTopology(windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

            // Root parameter 0 = viewport size as root constants.
            let vp = [window_w as f32, window_h as f32];
            cl.SetGraphicsRoot32BitConstants(0, 2, vp.as_ptr() as *const _, 0);

            // ── Chrome draw call (tab bar, sidebar, status bar) ───────────────
            if !chrome_cmds.is_empty() {
                let chrome_verts = ui_renderer::build_quads(chrome_cmds, &self.atlas);
                if !chrome_verts.is_empty() {
                    let needed = chrome_verts.len();
                    if self.chrome_buffer.as_ref().map_or(true, |b| b.capacity < needed) {
                        self.chrome_buffer = Some(
                            allocate_vertex_buffer(&self.ctx.device, needed * 2)?
                        );
                    }
                    let buf = self.chrome_buffer.as_mut().unwrap();
                    let byte_size = chrome_verts.len() * std::mem::size_of::<CellVertex>();
                    let mut ptr = std::ptr::null_mut();
                    buf.vb.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut ptr))
                        .context("Map chrome buffer")?;
                    std::ptr::copy_nonoverlapping(chrome_verts.as_ptr(), ptr as *mut CellVertex, chrome_verts.len());
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

            // Walk the pane layout tree and draw each pane.
            let (cx, cy, cw, ch) = content_rect
                .unwrap_or((0.0, 0.0, window_w as f32, window_h as f32));
            let pane_rects = layout.rects(cx, cy, cw, ch);
            self.pane_buffers.resize_with(pane_rects.len(), || {
                allocate_vertex_buffer(&self.ctx.device, MAX_CELLS_PER_PANE * VERTS_PER_CELL)
                    .expect("allocate vertex buffer")
            });

            for (pane_idx, (session_id, px, py, pw, ph)) in pane_rects.iter().enumerate() {
                let Some(session) = sessions.iter().find(|s| s.id == *session_id) else {
                    continue;
                };
                if !session.is_dirty() { continue; }

                let vertices = build_cell_quads(
                    &session.grid,
                    &self.atlas,
                    *px, *py, *pw, *ph,
                );

                if vertices.is_empty() { continue; }

                let buf = &mut self.pane_buffers[pane_idx];
                if vertices.len() > buf.capacity {
                    *buf = allocate_vertex_buffer(&self.ctx.device, vertices.len() * 2)?;
                }

                // Upload vertices.
                let byte_size = vertices.len() * std::mem::size_of::<CellVertex>();
                let mut ptr = std::ptr::null_mut();
                buf.vb.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut ptr))
                    .context("Map vertex buffer")?;
                std::ptr::copy_nonoverlapping(vertices.as_ptr(), ptr as *mut CellVertex, vertices.len());
                buf.vb.Unmap(0, None);

                let vb_addr = buf.vb.GetGPUVirtualAddress();
                let vbv = D3D12_VERTEX_BUFFER_VIEW {
                    BufferLocation: vb_addr,
                    SizeInBytes: byte_size as u32,
                    StrideInBytes: std::mem::size_of::<CellVertex>() as u32,
                };
                cl.IASetVertexBuffers(0, Some(&[vbv]));
                cl.DrawInstanced(vertices.len() as u32, 1, 0, 0);
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
}

/// Build a flat list of `CellVertex` quads (6 verts per cell) for the visible grid.
fn build_cell_quads(
    grid: &TerminalGrid,
    atlas: &GlyphAtlas,
    pane_x: f32,
    pane_y: f32,
    pane_w: f32,
    pane_h: f32,
) -> Vec<CellVertex> {
    let cols = grid.cols() as usize;
    let rows = grid.rows() as usize;

    let cw = pane_w / cols as f32;
    let ch = pane_h / rows as f32;

    let mut verts = Vec::with_capacity(cols * rows * VERTS_PER_CELL);

    for row in 0..rows {
        for col in 0..cols {
            let cell = match grid.cell(col as u16, row as u16) {
                Some(c) => c,
                None => continue,
            };

            let x0 = pane_x + col as f32 * cw;
            let y0 = pane_y + row as f32 * ch;
            let x1 = x0 + cw;
            let y1 = y0 + ch;

            let uv = atlas.uv_for_char(cell.ch);
            let fg = color_to_linear(&cell.fg);
            let bg = color_to_linear(&cell.bg);

            // Quad as two CCW triangles: TL, TR, BL, TR, BR, BL
            let tl = CellVertex { pos: [x0, y0], uv: [uv.u0, uv.v0], fg, bg };
            let tr = CellVertex { pos: [x1, y0], uv: [uv.u1, uv.v0], fg, bg };
            let bl = CellVertex { pos: [x0, y1], uv: [uv.u0, uv.v1], fg, bg };
            let br = CellVertex { pos: [x1, y1], uv: [uv.u1, uv.v1], fg, bg };

            verts.extend_from_slice(&[tl, tr, bl, tr, br, bl]);
        }
    }
    verts
}

fn allocate_vertex_buffer(device: &ID3D12Device, vertex_count: usize) -> Result<PaneBuffer> {
    let byte_size = (vertex_count * std::mem::size_of::<CellVertex>()) as u64;
    let vb: ID3D12Resource = unsafe {
        let mut vb_opt: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(
            &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() },
            D3D12_HEAP_FLAG_NONE,
            &D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: byte_size,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1, Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                Format: DXGI_FORMAT_UNKNOWN,
                ..Default::default()
            },
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut vb_opt,
        ).context("CreateCommittedResource (vertex buffer)")?;
        vb_opt.unwrap()
    };
    Ok(PaneBuffer { vb, capacity: vertex_count })
}

/// Map a `Color` to a linear-space RGBA float4.
fn color_to_linear(color: &Color) -> [f32; 4] {
    match color {
        Color::Default => [0.902, 0.929, 0.953, 1.0], // #e6edf3 foreground default
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
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Standard ANSI 256-colour palette (first 16 = system colours).
static ANSI_256: [[u8; 3]; 256] = {
    let mut t = [[0u8; 3]; 256];
    // Indices 0-7: standard colours
    t[0]  = [0,   0,   0  ]; // black
    t[1]  = [170, 0,   0  ]; // red
    t[2]  = [0,   170, 0  ]; // green
    t[3]  = [170, 170, 0  ]; // yellow
    t[4]  = [0,   0,   170]; // blue
    t[5]  = [170, 0,   170]; // magenta
    t[6]  = [0,   170, 170]; // cyan
    t[7]  = [170, 170, 170]; // white
    // 8-15: bright
    t[8]  = [85,  85,  85 ];
    t[9]  = [255, 85,  85 ];
    t[10] = [85,  255, 85 ];
    t[11] = [255, 255, 85 ];
    t[12] = [85,  85,  255];
    t[13] = [255, 85,  255];
    t[14] = [85,  255, 255];
    t[15] = [255, 255, 255];
    // 16-231: 6×6×6 colour cube
    let mut i = 16usize;
    let mut r = 0usize;
    while r < 6 { let mut g = 0usize; while g < 6 { let mut b = 0usize; while b < 6 {
        let rv = if r == 0 { 0 } else { 55 + r as u8 * 40 };
        let gv = if g == 0 { 0 } else { 55 + g as u8 * 40 };
        let bv = if b == 0 { 0 } else { 55 + b as u8 * 40 };
        t[i] = [rv, gv, bv]; i += 1;
        b += 1; } g += 1; } r += 1; }
    // 232-255: greyscale
    let mut k = 0usize;
    while k < 24 { t[232 + k] = [8 + k as u8 * 10; 3]; k += 1; }
    t
};
