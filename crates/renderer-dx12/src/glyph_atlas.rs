//! DirectWrite glyph rasteriser → R8_UNORM GPU texture atlas.
//!
//! The atlas has three sections packed into one texture:
//!
//!   Section 1 — Terminal font (Cascadia Code or requested family, monospace)
//!     Covers ASCII, Latin-1, Box Drawing, Block Elements, Geometric Shapes.
//!     All glyphs share the same (cell_w × cell_h) fixed-width cell.
//!
//!   Section 2 — UI font (Segoe UI Variable / Segoe UI, proportional)
//!     Covers ASCII printable (U+0020–U+007E).
//!     Glyphs are stored in fixed-width atlas cells but drawn at their actual
//!     advance widths, giving proportional text appearance.
//!
//!   Section 3 — Shape glyphs (1 row)
//!     Col 0: antialiased filled circle (elliptical in atlas → circular on screen)
//!     Cols 1-4: TL / TR / BL / BR quarter-circle corner pieces for rounded rects

use std::collections::HashMap;

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;

const ATLAS_RANGES: &[(u32, u32)] = &[
    (0x0020, 0x007E), // Basic ASCII printable (95)
    (0x00A0, 0x00FF), // Latin-1 Supplement   (96)
    (0x2500, 0x257F), // Box Drawing           (128)
    (0x2580, 0x259F), // Block Elements        (32)
    (0x25A0, 0x25CF), // Geometric Shapes      (48)
    (0xFFFD, 0xFFFD), // Replacement character (1)
];

const UI_CP_START: u32 = 0x0020;
const UI_CP_END: u32 = 0x007E;
const UI_FONT_SIZE_PT: f32 = 11.0;
const UI_FONT_FAMILIES: &[&str] = &["Segoe UI Variable", "Segoe UI", "Tahoma"];

/// Corner radius used for FillRoundRect (pixels on screen).
pub const CORNER_R: u32 = 4;

const ATLAS_COLS: u32 = 16;

fn terminal_codepoints() -> Vec<u32> {
    ATLAS_RANGES
        .iter()
        .flat_map(|&(lo, hi)| lo..=hi)
        .collect()
}

/// UV rectangle (0..1 texture space) for one glyph or shape.
#[derive(Debug, Clone, Copy, Default)]
pub struct GlyphUv {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

pub struct GlyphAtlas {
    pub texture: ID3D12Resource,
    // Terminal font
    pub cell_w: u32,
    pub cell_h: u32,
    pub atlas_w: u32,
    pub atlas_h: u32,
    uvs: HashMap<u32, GlyphUv>,
    fallback_uv: GlyphUv,
    // UI font
    pub ui_ascent: f32,
    /// UI font visual height (ascent + descent, in pixels). Always ≤ cell_h.
    pub ui_cell_h: u32,
    ui_uvs: HashMap<u32, GlyphUv>,
    pub ui_advances: HashMap<u32, f32>,
    // Shape glyphs
    pub circle_uv: GlyphUv,
    pub corner_uvs: [GlyphUv; 4], // [TL, TR, BL, BR]
}

impl GlyphAtlas {
    pub fn build(
        device: &ID3D12Device,
        upload_cmd_list: &ID3D12GraphicsCommandList,
        font_family: &str,
        font_size_pt: f32,
        dpi: f32,
    ) -> Result<(Self, ID3D12Resource)> {
        unsafe {
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).context("DWriteCreateFactory")?;

            // ── Section 1: Terminal font metrics ─────────────────────────────
            let font_size_dip = font_size_pt * dpi / 72.0;
            let font_family_w: Vec<u16> =
                font_family.encode_utf16().chain(std::iter::once(0)).collect();

            // Resolve the actual font face FIRST, then derive every metric
            // (cell_w, ascent, descent) from THAT face.  Measuring the advance
            // with CreateTextLayout instead lets DirectWrite silently substitute
            // a different (often proportional) family when the requested one is
            // missing, while rasterisation uses the fallback face — the two
            // disagree and glyphs get drawn at half the cell width.  Measuring
            // from the same face we rasterise with makes that mismatch impossible.
            let term_face = find_font_face(&dwrite, &font_family_w, "Consolas")?;
            let mut fm = DWRITE_FONT_METRICS::default();
            term_face.GetMetrics(&mut fm);
            let du = fm.designUnitsPerEm as f32;
            let ascent_dip = fm.ascent as f32 / du * font_size_dip;
            let descent_dip = fm.descent as f32 / du * font_size_dip;
            let cell_h = (ascent_dip + descent_dip).ceil() as u32;

            // Cell width = advance of 'M' in the resolved face (monospace, so
            // every glyph shares this advance).
            let mut m_index = [0u16; 1];
            term_face.GetGlyphIndices([b'M' as u32].as_ptr(), 1, m_index.as_mut_ptr()).ok();
            let mut m_metrics = [DWRITE_GLYPH_METRICS::default(); 1];
            term_face
                .GetDesignGlyphMetrics(m_index.as_ptr(), 1, m_metrics.as_mut_ptr(), false)
                .ok();
            let cell_w = (m_metrics[0].advanceWidth as f32 / du * font_size_dip).ceil() as u32;

            // ── Section 2: UI font metrics ────────────────────────────────────
            let ui_dip = UI_FONT_SIZE_PT * dpi / 72.0;
            let (ui_face, ui_family_w) = find_ui_font_face(&dwrite)?;
            let ui_format: IDWriteTextFormat = dwrite
                .CreateTextFormat(
                    windows::core::PCWSTR(ui_family_w.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    ui_dip,
                    windows::core::w!("en-us"),
                )
                .context("CreateTextFormat (UI)")?;
            let _ = ui_format;

            let mut ufm = DWRITE_FONT_METRICS::default();
            ui_face.GetMetrics(&mut ufm);
            let udu = ufm.designUnitsPerEm as f32;
            let ui_ascent_dip = ufm.ascent as f32 / udu * ui_dip;
            let ui_descent_dip = ufm.descent as f32 / udu * ui_dip;
            let ui_cell_h = (ui_ascent_dip + ui_descent_dip).ceil() as u32;
            let ui_ascent_px = ui_ascent_dip;

            // ── Atlas layout ──────────────────────────────────────────────────
            let term_cps = terminal_codepoints();
            let term_count = term_cps.len() as u32;
            let term_rows = (term_count + ATLAS_COLS - 1) / ATLAS_COLS;

            let ui_count = UI_CP_END - UI_CP_START + 1; // 95 chars
            let ui_rows = (ui_count + ATLAS_COLS - 1) / ATLAS_COLS; // 6 rows

            let shape_rows = 1u32; // circle (col 0) + 4 corners (cols 1-4)

            let total_rows = term_rows + ui_rows + shape_rows;
            let atlas_w = cell_w * ATLAS_COLS;
            let atlas_h = cell_h * total_rows;
            let mut atlas_data = vec![0u8; (atlas_w * atlas_h) as usize];

            let mut uvs: HashMap<u32, GlyphUv> = HashMap::with_capacity(term_count as usize);
            let mut fallback_uv = GlyphUv::default();
            let mut ui_uvs: HashMap<u32, GlyphUv> = HashMap::with_capacity(ui_count as usize);
            let mut ui_advances: HashMap<u32, f32> = HashMap::with_capacity(ui_count as usize);

            // ── Rasterise terminal glyphs ─────────────────────────────────────
            for (i, &cp) in term_cps.iter().enumerate() {
                let col = i as u32 % ATLAS_COLS;
                let row = i as u32 / ATLAS_COLS;
                let cell_x = col * cell_w;
                let cell_y = row * cell_h;

                let uv = cell_uv(cell_x, cell_y, cell_w, cell_h, atlas_w, atlas_h);
                uvs.insert(cp, uv);
                if cp == 0xFFFD {
                    fallback_uv = uv;
                }
                if cp == b' ' as u32 {
                    continue;
                }

                rasterize_glyph(
                    &dwrite,
                    &term_face,
                    cp,
                    font_size_dip,
                    ascent_dip,
                    cell_x,
                    cell_y,
                    cell_w,
                    cell_h,
                    atlas_w,
                    atlas_h,
                    &mut atlas_data,
                );
            }

            // ── Rasterise UI glyphs ───────────────────────────────────────────
            let ui_row_offset = term_rows;
            for (i, cp) in (UI_CP_START..=UI_CP_END).enumerate() {
                let col = i as u32 % ATLAS_COLS;
                let row = ui_row_offset + i as u32 / ATLAS_COLS;
                let cell_x = col * cell_w;
                let cell_y = row * cell_h;

                // Space: no glyph to rasterize; just record advance + UV.
                if cp == b' ' as u32 {
                    let adv = ui_dip * 0.28; // ~28% of em width for space
                    ui_uvs.insert(cp, GlyphUv {
                        u0: cell_x as f32 / atlas_w as f32,
                        v0: cell_y as f32 / atlas_h as f32,
                        u1: (cell_x as f32 + adv) / atlas_w as f32,
                        v1: (cell_y + cell_h) as f32 / atlas_h as f32,
                    });
                    ui_advances.insert(cp, adv);
                    continue;
                }

                // Resolve glyph index and advance from font metrics.
                let mut glyph_index = [0u16; 1];
                ui_face.GetGlyphIndices([cp].as_ptr(), 1, glyph_index.as_mut_ptr()).ok();

                let advance_px = if glyph_index[0] != 0 {
                    let mut gm = [DWRITE_GLYPH_METRICS::default(); 1];
                    ui_face
                        .GetDesignGlyphMetrics(glyph_index.as_ptr(), 1, gm.as_mut_ptr(), false)
                        .ok();
                    (gm[0].advanceWidth as f32 / udu * ui_dip).ceil()
                } else {
                    ui_dip * 0.6
                };

                // UV covers exactly the advance-width portion of the atlas cell.
                // This prevents the quad from squishing a narrow glyph into a
                // wider area when adv < cell_w.
                let uv = GlyphUv {
                    u0: cell_x as f32 / atlas_w as f32,
                    v0: cell_y as f32 / atlas_h as f32,
                    u1: (cell_x as f32 + advance_px.min(cell_w as f32)) / atlas_w as f32,
                    v1: (cell_y + cell_h) as f32 / atlas_h as f32,
                };
                ui_uvs.insert(cp, uv);
                ui_advances.insert(cp, advance_px);

                if glyph_index[0] == 0 {
                    continue;
                }

                rasterize_glyph(
                    &dwrite,
                    &ui_face,
                    cp,
                    ui_dip,
                    ui_ascent_px,
                    cell_x,
                    cell_y,
                    cell_w,
                    cell_h,
                    atlas_w,
                    atlas_h,
                    &mut atlas_data,
                );
            }

            // ── Rasterise shapes ──────────────────────────────────────────────
            let shape_row = term_rows + ui_rows;
            let shape_y = shape_row * cell_h;

            // Col 0: filled circle (elliptical in atlas, circular on screen)
            let circle_cell_x = 0u32;
            let circle_uv = cell_uv(circle_cell_x, shape_y, cell_w, cell_h, atlas_w, atlas_h);
            let cx_f = cell_w as f32 * 0.5;
            let cy_f = cell_h as f32 * 0.5;
            let rx = cell_w as f32 * 0.5 - 0.5;
            let ry = cell_h as f32 * 0.5 - 0.5;
            let edge_aa = 0.5 / rx.min(ry);
            for py in 0..cell_h {
                for px in 0..cell_w {
                    let nx = (px as f32 + 0.5 - cx_f) / rx;
                    let ny = (py as f32 + 0.5 - cy_f) / ry;
                    let dist = (nx * nx + ny * ny).sqrt();
                    let alpha = ((1.0 - dist + edge_aa).clamp(0.0, 1.0) * 255.0) as u8;
                    atlas_data[((shape_y + py) * atlas_w + circle_cell_x + px) as usize] = alpha;
                }
            }

            // Cols 1–4: corner quarter-circles (CORNER_R × CORNER_R pixels each)
            // Convention: alpha=1 where the rounded-rect SURFACE IS present.
            // TL (col 1): arc center at (R, R) from top-left of this sub-cell.
            // TR (col 2): arc center at (0, R).
            // BL (col 3): arc center at (R, 0).
            // BR (col 4): arc center at (0, 0).
            let r = CORNER_R as f32;
            let corner_centers: [(f32, f32); 4] = [(r, r), (0.0, r), (r, 0.0), (0.0, 0.0)];
            let mut corner_uvs = [GlyphUv::default(); 4];
            for (k, &(arc_cx, arc_cy)) in corner_centers.iter().enumerate() {
                let col = (k as u32) + 1;
                let cell_x = col * cell_w;
                // Corner quads are drawn at CORNER_R × CORNER_R pixels.
                // UV covers only the top-left CORNER_R × CORNER_R of the atlas cell.
                let u0 = cell_x as f32 / atlas_w as f32;
                let v0 = shape_y as f32 / atlas_h as f32;
                let u1 = (cell_x + CORNER_R) as f32 / atlas_w as f32;
                let v1 = (shape_y + CORNER_R) as f32 / atlas_h as f32;
                corner_uvs[k] = GlyphUv { u0, v0, u1, v1 };

                for py in 0..CORNER_R {
                    for px in 0..CORNER_R {
                        let dx = arc_cx - px as f32 - 0.5;
                        let dy = arc_cy - py as f32 - 0.5;
                        let dist = (dx * dx + dy * dy).sqrt();
                        let alpha = ((r - dist + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
                        atlas_data[((shape_y + py) * atlas_w + cell_x + px) as usize] = alpha;
                    }
                }
            }

            // ── Upload to GPU ─────────────────────────────────────────────────
            let texture_desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: atlas_w as u64,
                Height: atlas_h,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                ..Default::default()
            };
            let mut texture: Option<ID3D12Resource> = None;
            device
                .CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
                    D3D12_HEAP_FLAG_NONE,
                    &texture_desc,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut texture,
                )
                .context("CreateCommittedResource (atlas)")?;
            let texture = texture.unwrap();

            let row_pitch = align_up(atlas_w, D3D12_TEXTURE_DATA_PITCH_ALIGNMENT);
            let upload_size = (row_pitch * atlas_h) as u64;
            let mut upload_buf: Option<ID3D12Resource> = None;
            device
                .CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_UPLOAD,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &D3D12_RESOURCE_DESC {
                        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                        Width: upload_size,
                        Height: 1,
                        DepthOrArraySize: 1,
                        MipLevels: 1,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                        ..Default::default()
                    },
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                    None,
                    &mut upload_buf,
                )
                .context("CreateCommittedResource (atlas upload)")?;
            let upload_buf = upload_buf.unwrap();

            let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
            upload_buf
                .Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut mapped))
                .context("Map upload buffer")?;
            for y in 0..atlas_h {
                let src =
                    atlas_data[(y * atlas_w) as usize..((y + 1) * atlas_w) as usize].as_ptr();
                let dst = (mapped as *mut u8).add((y * row_pitch) as usize);
                std::ptr::copy_nonoverlapping(src, dst, atlas_w as usize);
            }
            upload_buf.Unmap(0, None);

            let src_loc = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(upload_buf.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: DXGI_FORMAT_R8_UNORM,
                            Width: atlas_w,
                            Height: atlas_h,
                            Depth: 1,
                            RowPitch: row_pitch,
                        },
                    },
                },
            };
            let dst_loc = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(texture.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            upload_cmd_list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, None);

            let barrier = crate::context::transition_barrier(
                &texture,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            );
            upload_cmd_list.ResourceBarrier(&[barrier]);

            Ok((
                Self {
                    texture,
                    cell_w,
                    cell_h,
                    atlas_w,
                    atlas_h,
                    uvs,
                    fallback_uv,
                    ui_ascent: ui_ascent_px,
                    ui_cell_h,
                    ui_uvs,
                    ui_advances,
                    circle_uv,
                    corner_uvs,
                },
                upload_buf,
            ))
        }
    }

    pub fn uv_for_char(&self, c: char) -> GlyphUv {
        self.uvs.get(&(c as u32)).copied().unwrap_or(self.fallback_uv)
    }

    /// UV + advance width (px) for a UI-font glyph.
    pub fn uv_for_ui_char(&self, c: char) -> Option<(GlyphUv, f32)> {
        let cp = c as u32;
        let uv = self.ui_uvs.get(&cp).copied()?;
        let adv = self.ui_advances.get(&cp).copied().unwrap_or(self.cell_w as f32 * 0.6);
        Some((uv, adv))
    }

    /// Total pixel width of a UI-font string.
    pub fn measure_ui_text(&self, text: &str) -> f32 {
        text.chars()
            .map(|c| {
                self.ui_advances
                    .get(&(c as u32))
                    .copied()
                    .unwrap_or(self.cell_w as f32 * 0.6)
            })
            .sum()
    }

    pub fn create_srv(&self, device: &ID3D12Device, heap: &ID3D12DescriptorHeap, slot: u32) {
        unsafe {
            let stride =
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            let base = heap.GetCPUDescriptorHandleForHeapStart();
            let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: base.ptr + (slot * stride) as usize,
            };
            let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R8_UNORM,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() },
                },
            };
            device.CreateShaderResourceView(Some(&self.texture), Some(&srv_desc), handle);
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn cell_uv(
    cell_x: u32,
    cell_y: u32,
    cell_w: u32,
    cell_h: u32,
    atlas_w: u32,
    atlas_h: u32,
) -> GlyphUv {
    GlyphUv {
        u0: cell_x as f32 / atlas_w as f32,
        v0: cell_y as f32 / atlas_h as f32,
        u1: (cell_x + cell_w) as f32 / atlas_w as f32,
        v1: (cell_y + cell_h) as f32 / atlas_h as f32,
    }
}

unsafe fn find_font_face(
    dwrite: &IDWriteFactory,
    family_w: &[u16],
    fallback: &str,
) -> Result<IDWriteFontFace> {
    let mut coll: Option<IDWriteFontCollection> = None;
    dwrite.GetSystemFontCollection(&mut coll, false).context("GetSystemFontCollection")?;
    let coll = coll.unwrap();

    let mut idx = 0u32;
    let mut found = windows::Win32::Foundation::BOOL::default();
    coll.FindFamilyName(windows::core::PCWSTR(family_w.as_ptr()), &mut idx, &mut found).ok();

    if !found.as_bool() {
        let fb: Vec<u16> = fallback.encode_utf16().chain(std::iter::once(0)).collect();
        coll.FindFamilyName(windows::core::PCWSTR(fb.as_ptr()), &mut idx, &mut found).ok();
    }

    let fam: IDWriteFontFamily = coll.GetFontFamily(idx).context("GetFontFamily")?;
    let font: IDWriteFont = fam
        .GetFirstMatchingFont(
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        )
        .context("GetFirstMatchingFont")?;
    font.CreateFontFace().context("CreateFontFace")
}

unsafe fn find_ui_font_face(dwrite: &IDWriteFactory) -> Result<(IDWriteFontFace, Vec<u16>)> {
    let mut coll: Option<IDWriteFontCollection> = None;
    dwrite.GetSystemFontCollection(&mut coll, false).context("GetSystemFontCollection")?;
    let coll = coll.unwrap();

    for &family in UI_FONT_FAMILIES {
        let wide: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
        let mut idx = 0u32;
        let mut found = windows::Win32::Foundation::BOOL::default();
        coll.FindFamilyName(windows::core::PCWSTR(wide.as_ptr()), &mut idx, &mut found).ok();
        if found.as_bool() {
            let fam: IDWriteFontFamily = coll.GetFontFamily(idx).context("GetFontFamily")?;
            let font: IDWriteFont = fam
                .GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                )
                .context("GetFirstMatchingFont")?;
            let face = font.CreateFontFace().context("CreateFontFace")?;
            return Ok((face, wide));
        }
    }
    // Absolute fallback: use any system font by asking for Tahoma.
    let wide: Vec<u16> = "Tahoma\0".encode_utf16().collect();
    let mut idx = 0u32;
    let mut found = windows::Win32::Foundation::BOOL::default();
    coll.FindFamilyName(windows::core::PCWSTR(wide.as_ptr()), &mut idx, &mut found).ok();
    let fam: IDWriteFontFamily = coll.GetFontFamily(0).context("GetFontFamily (fallback)")?;
    let font: IDWriteFont = fam
        .GetFirstMatchingFont(
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        )
        .context("GetFirstMatchingFont (fallback)")?;
    let face = font.CreateFontFace().context("CreateFontFace (fallback)")?;
    Ok((face, wide))
}

unsafe fn rasterize_glyph(
    dwrite: &IDWriteFactory,
    face: &IDWriteFontFace,
    cp: u32,
    font_size_dip: f32,
    ascent: f32,
    cell_x: u32,
    cell_y: u32,
    cell_w: u32,
    cell_h: u32,
    atlas_w: u32,
    atlas_h: u32,
    atlas_data: &mut [u8],
) {
    let mut glyph_index = [0u16; 1];
    let _ = face.GetGlyphIndices([cp].as_ptr(), 1, glyph_index.as_mut_ptr());
    if glyph_index[0] == 0 {
        return;
    }

    let mut fm = DWRITE_FONT_METRICS::default();
    face.GetMetrics(&mut fm);
    let du = fm.designUnitsPerEm as f32;
    let mut gm = [DWRITE_GLYPH_METRICS::default(); 1];
    let _ = face.GetDesignGlyphMetrics(glyph_index.as_ptr(), 1, gm.as_mut_ptr(), false);
    let advance = gm[0].advanceWidth as f32 / du * font_size_dip;

    let advance_arr = [advance];
    let offsets = [DWRITE_GLYPH_OFFSET::default()];
    let glyph_run = DWRITE_GLYPH_RUN {
        fontFace: std::mem::ManuallyDrop::new(Some(face.clone())),
        fontEmSize: font_size_dip,
        glyphCount: 1,
        glyphIndices: glyph_index.as_ptr(),
        glyphAdvances: advance_arr.as_ptr(),
        glyphOffsets: offsets.as_ptr(),
        isSideways: false.into(),
        bidiLevel: 0,
    };

    let analysis: IDWriteGlyphRunAnalysis = match dwrite.CreateGlyphRunAnalysis(
        &glyph_run,
        1.0,
        None,
        DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
        DWRITE_MEASURING_MODE_NATURAL,
        cell_x as f32,
        cell_y as f32 + ascent,
    ) {
        Ok(a) => a,
        Err(_) => return,
    };

    // ClearType 3×1: three per-subpixel coverage bytes per screen pixel.
    // They are NOT RGB colour — each is the coverage fraction for one LCD
    // subpixel (R, G, B).  Collapse to a single grayscale alpha by taking the
    // max of the three channels: this preserves stem width and gives the
    // sharpest single-channel equivalent.  The previous luma formula treated
    // coverage values as luminance, which is wrong and washes out thin strokes.
    let bounds = match analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_CLEARTYPE_3x1) {
        Ok(b) => b,
        Err(_) => return,
    };
    let bw = (bounds.right - bounds.left).max(0) as u32;
    let bh = (bounds.bottom - bounds.top).max(0) as u32;
    if bw == 0 || bh == 0 {
        return;
    }

    let mut pixels = vec![0u8; (bw * bh * 3) as usize];
    let _ = analysis.CreateAlphaTexture(DWRITE_TEXTURE_CLEARTYPE_3x1, &bounds, &mut pixels);

    for gy in 0..bh {
        let ay = bounds.top + gy as i32;
        if ay < cell_y as i32 || ay >= (cell_y + cell_h) as i32 || ay >= atlas_h as i32 {
            continue;
        }
        for gx in 0..bw {
            let ax = bounds.left + gx as i32;
            if ax < cell_x as i32 || ax >= (cell_x + cell_w) as i32 || ax >= atlas_w as i32 {
                continue;
            }
            let base = ((gy * bw + gx) * 3) as usize;
            // Max of R/G/B subpixel coverage → single grayscale alpha weight.
            let alpha = pixels[base].max(pixels[base + 1]).max(pixels[base + 2]);
            atlas_data[(ay as u32 * atlas_w + ax as u32) as usize] = alpha;
        }
    }
}

fn align_up(value: u32, align: u32) -> u32 {
    (value + align - 1) & !(align - 1)
}
