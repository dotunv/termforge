//! DirectWrite glyph rasteriser → R8_UNORM GPU texture atlas.
//!
//! Layout: 16-column grid of printable ASCII glyphs (char 0x20–0x7E, 95 chars).
//! Each cell is `cell_w × cell_h` pixels.  The atlas texture is
//! `ATLAS_COLS × cell_w` wide by `ceil(95 / ATLAS_COLS) × cell_h` tall.

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// Printable ASCII range stored in the atlas.
const FIRST_CHAR: u32 = 0x20; // space
const LAST_CHAR: u32 = 0x7E;  // tilde
const GLYPH_COUNT: u32 = LAST_CHAR - FIRST_CHAR + 1; // 95
const ATLAS_COLS: u32 = 16;
const ATLAS_ROWS: u32 = (GLYPH_COUNT + ATLAS_COLS - 1) / ATLAS_COLS; // 6

/// UV rectangle (in 0..1 texture space) for one glyph.
#[derive(Debug, Clone, Copy, Default)]
pub struct GlyphUv {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

pub struct GlyphAtlas {
    pub texture: ID3D12Resource,
    pub cell_w: u32,
    pub cell_h: u32,
    pub atlas_w: u32,
    pub atlas_h: u32,
    uvs: [GlyphUv; GLYPH_COUNT as usize],
}

impl GlyphAtlas {
    /// Build the atlas. `device` must be alive for the lifetime of the atlas.
    /// `upload_cmd_list` is the command list used to record the texture upload;
    /// the caller must execute and wait before using `texture` in shaders.
    pub fn build(
        device: &ID3D12Device,
        upload_cmd_list: &ID3D12GraphicsCommandList,
        font_family: &str,
        font_size_pt: f32,
        dpi: f32,
    ) -> Result<(Self, ID3D12Resource /* upload buffer, keep alive until exec */)> {
        unsafe {
            // ── DirectWrite setup ─────────────────────────────────────────────
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                    .context("DWriteCreateFactory")?;

            let font_size_dip = font_size_pt * dpi / 72.0;
            let font_family_w: Vec<u16> =
                font_family.encode_utf16().chain(std::iter::once(0)).collect();

            let text_format: IDWriteTextFormat = dwrite
                .CreateTextFormat(
                    windows::core::PCWSTR(font_family_w.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    font_size_dip,
                    windows::core::w!("en-us"),
                )
                .context("CreateTextFormat")?;

            // ── Measure cell dimensions from 'M' ─────────────────────────────
            let m_wide = [b'M' as u16];
            let layout: IDWriteTextLayout = dwrite
                .CreateTextLayout(
                    &m_wide,
                    &text_format,
                    1000.0,
                    1000.0,
                )
                .context("CreateTextLayout (M)")?;

            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics).context("GetMetrics")?;

            // Cell width = advance; cell height = line height. Round up to int.
            let cell_w = metrics.width.ceil() as u32 + 1;
            let cell_h = metrics.height.ceil() as u32;

            let atlas_w = cell_w * ATLAS_COLS;
            let atlas_h = cell_h * ATLAS_ROWS;

            // ── Rasterise all glyphs into a CPU buffer ────────────────────────
            let atlas_pixels = atlas_w * atlas_h;
            let mut atlas_data = vec![0u8; atlas_pixels as usize];

            // Get font face for glyph run analysis.
            let mut font_coll: Option<IDWriteFontCollection> = None;
            dwrite
                .GetSystemFontCollection(&mut font_coll, false)
                .context("GetSystemFontCollection")?;
            let font_coll = font_coll.unwrap();

            let mut family_idx = 0u32;
            let mut exists = windows::Win32::Foundation::BOOL::default();
            font_coll
                .FindFamilyName(
                    windows::core::PCWSTR(font_family_w.as_ptr()),
                    &mut family_idx,
                    &mut exists,
                )
                .context("FindFamilyName")?;

            // Fallback to "Courier New" if the requested font isn't found.
            if !exists.as_bool() {
                font_coll
                    .FindFamilyName(
                        windows::core::w!("Courier New"),
                        &mut family_idx,
                        &mut exists,
                    )
                    .ok();
            }

            let font_family_obj: IDWriteFontFamily = font_coll
                .GetFontFamily(family_idx)
                .context("GetFontFamily")?;
            let font: IDWriteFont = font_family_obj
                .GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                )
                .context("GetFirstMatchingFont")?;
            let font_face: IDWriteFontFace = font.CreateFontFace().context("CreateFontFace")?;

            // Font metrics for baseline calculation.
            let mut font_metrics = DWRITE_FONT_METRICS::default();
            font_face.GetMetrics(&mut font_metrics);
            let design_units = font_metrics.designUnitsPerEm as f32;
            let ascent_dip = font_metrics.ascent as f32 / design_units * font_size_dip;

            let mut uvs = [GlyphUv::default(); GLYPH_COUNT as usize];

            for (i, cp) in (FIRST_CHAR..=LAST_CHAR).enumerate() {
                let col = i as u32 % ATLAS_COLS;
                let row = i as u32 / ATLAS_COLS;
                let cell_x = col * cell_w;
                let cell_y = row * cell_h;

                // Store UV.
                uvs[i] = GlyphUv {
                    u0: cell_x as f32 / atlas_w as f32,
                    v0: cell_y as f32 / atlas_h as f32,
                    u1: (cell_x + cell_w) as f32 / atlas_w as f32,
                    v1: (cell_y + cell_h) as f32 / atlas_h as f32,
                };

                // Space glyph — leave blank in atlas.
                if cp == b' ' as u32 { continue; }

                // Resolve glyph index.
                let codepoints = [cp];
                let mut glyph_index = [0u16; 1];
                font_face
                    .GetGlyphIndices(codepoints.as_ptr(), 1, glyph_index.as_mut_ptr())
                    .context("GetGlyphIndices")?;

                if glyph_index[0] == 0 { continue; }

                // Glyph metrics for advance.
                let mut glyph_metrics = [DWRITE_GLYPH_METRICS::default(); 1];
                font_face
                    .GetDesignGlyphMetrics(glyph_index.as_ptr(), 1, glyph_metrics.as_mut_ptr(), false)
                    .context("GetDesignGlyphMetrics")?;
                let advance = glyph_metrics[0].advanceWidth as f32 / design_units * font_size_dip;

                let advance_arr = [advance];
                let glyph_offsets = [DWRITE_GLYPH_OFFSET::default(); 1];

                let glyph_run = DWRITE_GLYPH_RUN {
                    fontFace: std::mem::ManuallyDrop::new(Some(font_face.clone())),
                    fontEmSize: font_size_dip,
                    glyphCount: 1,
                    glyphIndices: glyph_index.as_ptr(),
                    glyphAdvances: advance_arr.as_ptr(),
                    glyphOffsets: glyph_offsets.as_ptr(),
                    isSideways: false.into(),
                    bidiLevel: 0,
                };

                // Origin: left edge of cell, baseline from ascent.
                let origin_x = cell_x as f32;
                let origin_y = cell_y as f32 + ascent_dip;

                let analysis: IDWriteGlyphRunAnalysis = dwrite
                    .CreateGlyphRunAnalysis(
                        &glyph_run,
                        1.0, // pixels per DIP
                        None,
                        DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
                        DWRITE_MEASURING_MODE_NATURAL,
                        origin_x,
                        origin_y,
                    )
                    .context("CreateGlyphRunAnalysis")?;

                let bounds = analysis
                    .GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)
                    .context("GetAlphaTextureBounds")?;

                let bw = (bounds.right - bounds.left).max(0) as u32;
                let bh = (bounds.bottom - bounds.top).max(0) as u32;
                if bw == 0 || bh == 0 { continue; }

                let mut glyph_pixels = vec![0u8; (bw * bh) as usize];
                analysis
                    .CreateAlphaTexture(
                        DWRITE_TEXTURE_ALIASED_1x1,
                        &bounds,
                        &mut glyph_pixels,
                    )
                    .context("CreateAlphaTexture")?;

                // Blit glyph_pixels into atlas_data, clipped to cell bounds.
                let dst_x = bounds.left.max(cell_x as i32) as u32;
                let dst_y = bounds.top.max(cell_y as i32) as u32;

                for gy in 0..bh {
                    let ay = dst_y + gy;
                    if ay >= cell_y + cell_h || ay >= atlas_h { break; }
                    for gx in 0..bw {
                        let ax = dst_x + gx;
                        if ax >= cell_x + cell_w || ax >= atlas_w { break; }
                        atlas_data[(ay * atlas_w + ax) as usize] =
                            glyph_pixels[(gy * bw + gx) as usize];
                    }
                }
            }

            // ── GPU texture (default heap, R8_UNORM) ──────────────────────────
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
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
                D3D12_HEAP_FLAG_NONE,
                &texture_desc,
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut texture,
            ).context("CreateCommittedResource (atlas)")?;
            let texture = texture.unwrap();

            // ── Upload buffer ─────────────────────────────────────────────────
            // Row pitch must be aligned to D3D12_TEXTURE_DATA_PITCH_ALIGNMENT (256).
            let row_pitch = align_up(atlas_w, D3D12_TEXTURE_DATA_PITCH_ALIGNMENT);
            let upload_size = (row_pitch * atlas_h) as u64;

            let mut upload_buf: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() },
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
            ).context("CreateCommittedResource (atlas upload)")?;
            let upload_buf = upload_buf.unwrap();

            // Map upload buffer and fill row-by-row (accounting for row pitch padding).
            let mut mapped_ptr = std::ptr::null_mut();
            upload_buf.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut mapped_ptr))
                .context("Map upload buffer")?;
            for y in 0..atlas_h {
                let src = atlas_data[(y * atlas_w) as usize..((y + 1) * atlas_w) as usize].as_ptr();
                let dst = (mapped_ptr as *mut u8).add((y * row_pitch) as usize);
                std::ptr::copy_nonoverlapping(src, dst, atlas_w as usize);
            }
            upload_buf.Unmap(0, None);

            // Record CopyTextureRegion into the upload command list.
            let src = D3D12_TEXTURE_COPY_LOCATION {
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
            let dst = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(texture.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            upload_cmd_list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);

            // Transition atlas texture: COPY_DEST → PIXEL_SHADER_RESOURCE.
            let barrier = crate::context::transition_barrier(
                &texture,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            );
            upload_cmd_list.ResourceBarrier(&[barrier]);

            Ok((
                Self { texture, cell_w, cell_h, atlas_w, atlas_h, uvs },
                upload_buf,
            ))
        }
    }

    /// UV rect for a printable ASCII char. Returns a space UV for out-of-range chars.
    pub fn uv_for_char(&self, c: char) -> GlyphUv {
        let cp = c as u32;
        if cp >= FIRST_CHAR && cp <= LAST_CHAR {
            self.uvs[(cp - FIRST_CHAR) as usize]
        } else {
            self.uvs[0] // space
        }
    }

    /// Create an SRV for the atlas texture in the given heap slot.
    pub fn create_srv(&self, device: &ID3D12Device, heap: &ID3D12DescriptorHeap, slot: u32) {
        unsafe {
            let stride = device.GetDescriptorHandleIncrementSize(
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            );
            let base = heap.GetCPUDescriptorHandleForHeapStart();
            let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: base.ptr + (slot * stride) as usize,
            };
            let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R8_UNORM,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            };
            device.CreateShaderResourceView(Some(&self.texture), Some(&srv_desc), handle);
        }
    }
}

fn align_up(value: u32, align: u32) -> u32 {
    (value + align - 1) & !(align - 1)
}
