use std::sync::mpsc;

use anyhow::{Context, Result};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Messages sent from the WndProc back to the main loop.
#[derive(Debug)]
pub enum WindowEvent {
    Char(u16),
    KeyDown { vk: u32, ctrl: bool },
    Resize { width: u32, height: u32 },
    /// Raw mouse position in client coordinates.
    MouseMove { x: i32, y: i32 },
    LButtonDown { x: i32, y: i32 },
    LButtonUp,
    Close,
}

/// Owns the Win32 HWND and manages window class registration.
pub struct Window {
    pub hwnd: HWND,
}

struct WindowState {
    tx: mpsc::SyncSender<WindowEvent>,
}

impl Window {
    pub fn new(
        title: &str,
        width: u32,
        height: u32,
        event_tx: mpsc::SyncSender<WindowEvent>,
    ) -> Result<Self> {
        unsafe {
            let hinstance: HINSTANCE = GetModuleHandleW(None)
                .context("GetModuleHandleW")?
                .into();

            let class_name = w!("TermForgeWindow");

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: class_name,
                ..Default::default()
            };

            RegisterClassExW(&wc);

            let state = Box::new(WindowState { tx: event_tx });
            let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                windows::core::PCWSTR(title_wide.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width as i32,
                height as i32,
                None,
                None,
                hinstance,
                Some(Box::into_raw(state) as *const _),
            ).context("CreateWindowExW")?;

            let _ = ShowWindow(hwnd, SW_SHOW);
            Ok(Self { hwnd })
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let create_struct = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create_struct.lpCreateParams as isize);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        WM_DESTROY => {
            let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if ptr != 0 {
                drop(Box::from_raw(ptr as *mut WindowState));
            }
            PostQuitMessage(0);
            LRESULT(0)
        }

        WM_CLOSE => {
            send_event(hwnd, WindowEvent::Close);
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }

        WM_SIZE => {
            let width  = (lparam.0 & 0xFFFF) as u32;
            let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
            if width > 0 && height > 0 {
                send_event(hwnd, WindowEvent::Resize { width, height });
            }
            LRESULT(0)
        }

        WM_CHAR => {
            send_event(hwnd, WindowEvent::Char(wparam.0 as u16));
            LRESULT(0)
        }

        WM_KEYDOWN => {
            let vk   = wparam.0 as u32;
            let ctrl = GetKeyState(0x11) as i16 & (0x8000u16 as i16) != 0;
            send_event(hwnd, WindowEvent::KeyDown { vk, ctrl });
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            send_event(hwnd, WindowEvent::MouseMove { x, y });
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            // Capture mouse so we continue receiving events during drag.
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd);
            send_event(hwnd, WindowEvent::LButtonDown { x, y });
            LRESULT(0)
        }

        WM_LBUTTONUP => {
            windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture().ok();
            send_event(hwnd, WindowEvent::LButtonUp);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn send_event(hwnd: HWND, event: WindowEvent) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if ptr == 0 { return; }
    let state = &*(ptr as *const WindowState);
    let _ = state.tx.try_send(event);
}
