//! Root signature, PSO, and vertex layout for the cell renderer.
//!
//! Each terminal cell is rendered as two triangles (quad).
//! The vertex shader converts pixel coordinates to NDC.
//! The pixel shader samples the R8 glyph atlas and lerps fg/bg.

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D::Fxc::{
    D3DCompile, D3DCOMPILE_DEBUG, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCOMPILE_SKIP_OPTIMIZATION,
};
use windows::Win32::Graphics::Direct3D12::D3D_ROOT_SIGNATURE_VERSION_1;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// One vertex of a cell quad.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CellVertex {
    /// Pixel-space position (origin = top-left of window).
    pub pos: [f32; 2],
    /// UV coordinates into the glyph atlas (0..1).
    pub uv: [f32; 2],
    /// Foreground colour (RGBA, linear).
    pub fg: [f32; 4],
    /// Background colour (RGBA, linear).
    pub bg: [f32; 4],
}

// 48 bytes per vertex: 2+2+4+4 floats.
const _: () = assert!(std::mem::size_of::<CellVertex>() == 48);

const VERTEX_SHADER: &str = r#"
cbuffer FrameConstants : register(b0) {
    float2 viewport_size;
};

struct VS_IN {
    float2 pos  : POSITION;
    float2 uv   : TEXCOORD0;
    float4 fg   : COLOR0;
    float4 bg   : COLOR1;
};

struct VS_OUT {
    float4 pos  : SV_POSITION;
    float2 uv   : TEXCOORD0;
    float4 fg   : COLOR0;
    float4 bg   : COLOR1;
};

VS_OUT main(VS_IN input) {
    VS_OUT o;
    float2 ndc = (input.pos / viewport_size) * 2.0 - 1.0;
    ndc.y = -ndc.y;
    o.pos = float4(ndc, 0.0, 1.0);
    o.uv  = input.uv;
    o.fg  = input.fg;
    o.bg  = input.bg;
    return o;
}
"#;

const PIXEL_SHADER: &str = r#"
Texture2D<float> glyph_atlas : register(t0);
SamplerState     samp        : register(s0);

struct PS_IN {
    float4 pos : SV_POSITION;
    float2 uv  : TEXCOORD0;
    float4 fg  : COLOR0;
    float4 bg  : COLOR1;
};

float4 main(PS_IN input) : SV_TARGET {
    float alpha = glyph_atlas.Sample(samp, input.uv);
    return lerp(input.bg, input.fg, alpha);
}
"#;

pub struct RenderPipeline {
    pub root_signature: ID3D12RootSignature,
    pub pso: ID3D12PipelineState,
}

impl RenderPipeline {
    pub fn new(device: &ID3D12Device) -> Result<Self> {
        unsafe {
            let root_signature = create_root_signature(device)?;
            let pso = create_pso(device, &root_signature)?;
            Ok(Self {
                root_signature,
                pso,
            })
        }
    }
}

unsafe fn create_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature> {
    // Root parameter 0: 2 × 32-bit root constants (viewport width, height) → b0, VS-only.
    // Root parameter 1: descriptor table — 1 SRV range (glyph atlas) → t0, PS-only.
    let srv_range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };

    let params = [
        // Root constants for viewport size.
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: 2,
                },
            },
        },
        // Descriptor table for the glyph atlas SRV.
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &srv_range,
                },
            },
        },
    ];

    // Static sampler (point / linear for glyph atlas — bilinear looks good at non-integer scales).
    let sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        ..Default::default()
    };

    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 1,
        pStaticSamplers: &sampler,
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    };

    let mut blob: Option<windows::Win32::Graphics::Direct3D::ID3DBlob> = None;
    let mut error_blob: Option<windows::Win32::Graphics::Direct3D::ID3DBlob> = None;
    D3D12SerializeRootSignature(
        &desc,
        D3D_ROOT_SIGNATURE_VERSION_1,
        &mut blob,
        Some(&mut error_blob),
    )
    .map_err(|e| {
        if let Some(err) = &error_blob {
            let msg = unsafe {
                let ptr = err.GetBufferPointer() as *const u8;
                let len = err.GetBufferSize();
                std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).unwrap_or("?")
            };
            anyhow::anyhow!("D3D12SerializeRootSignature: {e} — {msg}")
        } else {
            anyhow::anyhow!("D3D12SerializeRootSignature: {e}")
        }
    })?;

    let blob = blob.unwrap();
    device
        .CreateRootSignature(
            0,
            std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()),
        )
        .context("CreateRootSignature")
}

unsafe fn create_pso(
    device: &ID3D12Device,
    root_sig: &ID3D12RootSignature,
) -> Result<ID3D12PipelineState> {
    let vs_blob = compile_shader(VERTEX_SHADER, "main", "vs_5_0")?;
    let ps_blob = compile_shader(PIXEL_SHADER, "main", "ps_5_0")?;

    let vs_bytecode = D3D12_SHADER_BYTECODE {
        pShaderBytecode: vs_blob.GetBufferPointer(),
        BytecodeLength: vs_blob.GetBufferSize(),
    };
    let ps_bytecode = D3D12_SHADER_BYTECODE {
        pShaderBytecode: ps_blob.GetBufferPointer(),
        BytecodeLength: ps_blob.GetBufferSize(),
    };

    // Input layout matching CellVertex.
    let position_sem = windows::core::s!("POSITION");
    let texcoord_sem = windows::core::s!("TEXCOORD");
    let color_sem = windows::core::s!("COLOR");

    let input_layout = [
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: position_sem,
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 0,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: texcoord_sem,
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 8,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: color_sem,
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 16,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: color_sem,
            SemanticIndex: 1,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 32,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ];

    // Alpha blend: fg drawn on top of bg using glyph alpha.
    let blend_rt = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: D3D12_BLEND_SRC_ALPHA,
        DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_ZERO,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
        ..Default::default()
    };
    let mut blend_desc = D3D12_BLEND_DESC::default();
    blend_desc.RenderTarget[0] = blend_rt;

    let pso_desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: std::mem::ManuallyDrop::new(Some(root_sig.clone())),
        VS: vs_bytecode,
        PS: ps_bytecode,
        BlendState: blend_desc,
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            ..Default::default()
        },
        DepthStencilState: D3D12_DEPTH_STENCIL_DESC::default(),
        InputLayout: D3D12_INPUT_LAYOUT_DESC {
            pInputElementDescs: input_layout.as_ptr(),
            NumElements: input_layout.len() as u32,
        },
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        RTVFormats: {
            let mut arr = [DXGI_FORMAT_UNKNOWN; 8];
            arr[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
            arr
        },
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        SampleMask: u32::MAX,
        ..Default::default()
    };

    device
        .CreateGraphicsPipelineState(&pso_desc)
        .context("CreateGraphicsPipelineState")
}

unsafe fn compile_shader(
    source: &str,
    entry: &str,
    target: &str,
) -> Result<windows::Win32::Graphics::Direct3D::ID3DBlob> {
    let entry_cstr = std::ffi::CString::new(entry).unwrap();
    let target_cstr = std::ffi::CString::new(target).unwrap();

    let mut code: Option<windows::Win32::Graphics::Direct3D::ID3DBlob> = None;
    let mut errors: Option<windows::Win32::Graphics::Direct3D::ID3DBlob> = None;

    let flags = if cfg!(debug_assertions) {
        D3DCOMPILE_DEBUG | D3DCOMPILE_SKIP_OPTIMIZATION
    } else {
        D3DCOMPILE_OPTIMIZATION_LEVEL3
    };

    D3DCompile(
        source.as_ptr() as *const _,
        source.len(),
        windows::core::PCSTR::null(),
        None,
        None,
        windows::core::PCSTR(entry_cstr.as_ptr() as *const u8),
        windows::core::PCSTR(target_cstr.as_ptr() as *const u8),
        flags,
        0,
        &mut code,
        Some(&mut errors),
    )
    .map_err(|e| {
        if let Some(err) = &errors {
            let ptr = err.GetBufferPointer() as *const u8;
            let len = err.GetBufferSize();
            let msg = std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).unwrap_or("?");
            anyhow::anyhow!("D3DCompile({target}): {e}\n{msg}")
        } else {
            anyhow::anyhow!("D3DCompile({target}): {e}")
        }
    })?;

    Ok(code.unwrap())
}
