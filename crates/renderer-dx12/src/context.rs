use anyhow::{Context, Result};
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, HANDLE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::Foundation::HWND;

/// Number of swap-chain back buffers (double-buffered).
pub const FRAME_COUNT: usize = 2;

/// All DX12 per-frame state for one swap chain.
struct FrameResources {
    /// `None` only during the window between releasing buffers and recreating
    /// them in `resize()`.  Always `Some` during normal rendering.
    render_target: Option<ID3D12Resource>,
    cmd_allocator: ID3D12CommandAllocator,
    fence_value: u64,
}

/// The single shared GPU context that all sessions render into.
pub struct Dx12Context {
    pub device: ID3D12Device,
    pub cmd_queue: ID3D12CommandQueue,
    pub cmd_list: ID3D12GraphicsCommandList,

    swap_chain: IDXGISwapChain3,
    frames: [FrameResources; FRAME_COUNT],
    frame_index: usize,

    rtv_heap: ID3D12DescriptorHeap,
    rtv_stride: u32,

    /// CBV/SRV/UAV heap (shader-visible) — slot 0 = glyph atlas SRV.
    pub srv_heap: ID3D12DescriptorHeap,

    fence: ID3D12Fence,
    fence_event: HANDLE,
    next_fence_value: u64,

    pub width: u32,
    pub height: u32,
    pub viewport: D3D12_VIEWPORT,
    pub scissor: RECT,
}

impl Dx12Context {
    pub fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self> {
        unsafe {
            // ── Device ───────────────────────────────────────────────────────
            #[cfg(debug_assertions)]
            {
                // Enable D3D12 debug layer in debug builds.
                let mut debug: Option<ID3D12Debug> = None;
                if D3D12GetDebugInterface(&mut debug).is_ok() {
                    if let Some(d) = debug { d.EnableDebugLayer(); }
                }
            }

            let mut device: Option<ID3D12Device> = None;
            D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device)
                .context("D3D12CreateDevice")?;
            let device = device.unwrap();

            // ── Command queue ────────────────────────────────────────────────
            let cmd_queue: ID3D12CommandQueue = device.CreateCommandQueue(
                &D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                },
            ).context("CreateCommandQueue")?;

            // ── DXGI factory + swap chain ─────────────────────────────────────
            let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))
                .context("CreateDXGIFactory2")?;

            let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                BufferCount: FRAME_COUNT as u32,
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };

            let swap_chain: IDXGISwapChain3 = factory
                .CreateSwapChainForHwnd(&cmd_queue, hwnd, &sc_desc, None, None)
                .context("CreateSwapChainForHwnd")?
                .cast()
                .context("IDXGISwapChain3 cast")?;

            // Disable Alt+Enter fullscreen toggle (we manage this ourselves).
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER).ok();

            // ── RTV descriptor heap ───────────────────────────────────────────
            let rtv_heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(
                &D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: FRAME_COUNT as u32,
                    ..Default::default()
                },
            ).context("CreateDescriptorHeap (RTV)")?;
            let rtv_stride = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV);
            let rtv_base = rtv_heap.GetCPUDescriptorHandleForHeapStart();

            // ── SRV heap (shader-visible) — slot 0 reserved for glyph atlas ──
            let srv_heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(
                &D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 16, // plenty for Phase 2
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    ..Default::default()
                },
            ).context("CreateDescriptorHeap (SRV)")?;

            // ── Per-frame resources ───────────────────────────────────────────
            let frame_index = swap_chain.GetCurrentBackBufferIndex() as usize;

            let frames = std::array::from_fn(|i| {
                let render_target: ID3D12Resource = swap_chain.GetBuffer(i as u32).unwrap();
                let rtv_handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: rtv_base.ptr + i * rtv_stride as usize,
                };
                device.CreateRenderTargetView(&render_target, None, rtv_handle);

                let cmd_allocator: ID3D12CommandAllocator = device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                    .unwrap();

                FrameResources { render_target: Some(render_target), cmd_allocator, fence_value: 0 }
            });

            // ── Command list (starts closed — caller must Reset before use) ──
            let cmd_list: ID3D12GraphicsCommandList = device.CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                &frames[frame_index].cmd_allocator,
                None,
            ).context("CreateCommandList")?;
            cmd_list.Close().context("initial cmd_list Close")?;

            // ── Fence ────────────────────────────────────────────────────────
            let fence: ID3D12Fence = device
                .CreateFence(0, D3D12_FENCE_FLAG_NONE)
                .context("CreateFence")?;
            let fence_event = CreateEventW(None, false, false, None)
                .context("CreateEventW")?;

            let viewport = D3D12_VIEWPORT {
                Width: width as f32,
                Height: height as f32,
                MaxDepth: 1.0,
                ..Default::default()
            };
            let scissor = RECT { right: width as i32, bottom: height as i32, ..Default::default() };

            Ok(Self {
                device,
                cmd_queue,
                cmd_list,
                swap_chain,
                frames,
                frame_index,
                rtv_heap,
                rtv_stride,
                srv_heap,
                fence,
                fence_event,
                next_fence_value: 1,
                width,
                height,
                viewport,
                scissor,
            })
        }
    }

    /// Begin recording a frame. Returns the current back-buffer RTV handle.
    pub fn begin_frame(&mut self) -> Result<D3D12_CPU_DESCRIPTOR_HANDLE> {
        unsafe {
            let frame = &self.frames[self.frame_index];
            // Wait if GPU is still using this allocator's commands.
            if self.fence.GetCompletedValue() < frame.fence_value {
                self.fence.SetEventOnCompletion(frame.fence_value, self.fence_event)
                    .context("SetEventOnCompletion")?;
                WaitForSingleObject(self.fence_event, u32::MAX);
            }

            frame.cmd_allocator.Reset().context("cmd_allocator Reset")?;
            self.cmd_list.Reset(&frame.cmd_allocator, None).context("cmd_list Reset")?;

            // Transition back buffer: PRESENT → RENDER_TARGET
            let barrier = transition_barrier(
                self.frames[self.frame_index].render_target.as_ref().unwrap(),
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            );
            self.cmd_list.ResourceBarrier(&[barrier]);

            let rtv_base = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
            let rtv = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: rtv_base.ptr + self.frame_index * self.rtv_stride as usize,
            };
            Ok(rtv)
        }
    }

    /// Finish recording, execute, present, and advance the fence.
    pub fn end_frame(&mut self) -> Result<()> {
        unsafe {
            // Transition: RENDER_TARGET → PRESENT
            let barrier = transition_barrier(
                self.frames[self.frame_index].render_target.as_ref().unwrap(),
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PRESENT,
            );
            self.cmd_list.ResourceBarrier(&[barrier]);
            self.cmd_list.Close().context("cmd_list Close")?;

            // Execute
            let cmd_list_ptr: ID3D12CommandList = self.cmd_list.cast().unwrap();
            self.cmd_queue.ExecuteCommandLists(&[Some(cmd_list_ptr)]);

            // Present (vsync interval = 1)
            let _ = self.swap_chain.Present(1, DXGI_PRESENT(0)).ok();

            // Signal fence for this frame
            let signal_value = self.next_fence_value;
            self.next_fence_value += 1;
            self.cmd_queue.Signal(&self.fence, signal_value)
                .context("Signal")?;
            self.frames[self.frame_index].fence_value = signal_value;

            // Advance frame index
            self.frame_index = self.swap_chain.GetCurrentBackBufferIndex() as usize;
            Ok(())
        }
    }

    /// Resize swap chain and recreate RTVs. Call on WM_SIZE.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        unsafe {
            // Wait for GPU idle before releasing resources.
            self.flush_gpu()?;

            // CRITICAL: set each slot to None so the COM ref-count drops to zero
            // before ResizeBuffers is called.  Any live ID3D12Resource reference
            // to a swap-chain buffer causes DXGI_ERROR_INVALID_CALL (0x887A0001).
            for frame in &mut self.frames {
                frame.render_target = None;
            }

            self.swap_chain.ResizeBuffers(
                FRAME_COUNT as u32,
                width,
                height,
                DXGI_FORMAT_UNKNOWN, // preserve format
                DXGI_SWAP_CHAIN_FLAG(0),
            ).context("ResizeBuffers")?;

            let rtv_base = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
            for (i, frame) in self.frames.iter_mut().enumerate() {
                frame.render_target = Some(self.swap_chain.GetBuffer(i as u32)
                    .context("GetBuffer after resize")?);
                self.device.CreateRenderTargetView(
                    frame.render_target.as_ref().unwrap(),
                    None,
                    D3D12_CPU_DESCRIPTOR_HANDLE {
                        ptr: rtv_base.ptr + i * self.rtv_stride as usize,
                    },
                );
            }

            self.frame_index = self.swap_chain.GetCurrentBackBufferIndex() as usize;
            self.width = width;
            self.height = height;
            self.viewport = D3D12_VIEWPORT {
                Width: width as f32,
                Height: height as f32,
                MaxDepth: 1.0,
                ..Default::default()
            };
            self.scissor = RECT { right: width as i32, bottom: height as i32, ..Default::default() };
            Ok(())
        }
    }

    /// Wait for all in-flight GPU work to complete.
    pub fn flush_gpu(&mut self) -> Result<()> {
        unsafe {
            let value = self.next_fence_value;
            self.next_fence_value += 1;
            self.cmd_queue.Signal(&self.fence, value).context("flush Signal")?;
            if self.fence.GetCompletedValue() < value {
                self.fence.SetEventOnCompletion(value, self.fence_event)
                    .context("flush SetEventOnCompletion")?;
                WaitForSingleObject(self.fence_event, u32::MAX);
            }
            Ok(())
        }
    }
}

impl Drop for Dx12Context {
    fn drop(&mut self) {
        let _ = self.flush_gpu();
        unsafe { CloseHandle(self.fence_event).ok(); }
    }
}

/// Build a resource transition barrier (avoids the verbose union syntax every call).
pub fn transition_barrier(
    resource: &ID3D12Resource,
    state_before: D3D12_RESOURCE_STATES,
    state_after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(resource) },
                StateBefore: state_before,
                StateAfter: state_after,
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
            }),
        },
    }
}
