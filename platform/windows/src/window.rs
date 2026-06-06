use std::sync::mpsc;

use anyhow::{Context, Result};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmIsCompositionEnabled};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Width of the invisible resize border (pixels). Matches OS default.
const RESIZE_BORDER: i32 = 8;

/// Messages sent from the WndProc back to the main loop.
#[derive(Debug)]
pub enum WindowEvent {
    Char(u16),
    KeyDown {
        vk: u32,
        ctrl: bool,
    },
    Resize {
        width: u32,
        height: u32,
    },
    /// Raw mouse position in client coordinates.
    MouseMove {
        x: i32,
        y: i32,
    },
    LButtonDown {
        x: i32,
        y: i32,
    },
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
            let hinstance: HINSTANCE = GetModuleHandleW(None).context("GetModuleHandleW")?.into();

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

            // WS_THICKFRAME kept for OS resize; WS_CAPTION removed so our custom
            // 36px titlebar is the only title bar. WM_NCCALCSIZE zeroes the NC
            // area so the DX12 surface covers the full window rect.
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                windows::core::PCWSTR(title_wide.as_ptr()),
                WS_OVERLAPPEDWINDOW & !WS_CAPTION,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width as i32,
                height as i32,
                None,
                None,
                hinstance,
                Some(Box::into_raw(state) as *const _),
            )
            .context("CreateWindowExW")?;

            // Extend the DWM frame into the client area with negative margins so
            // the DWM still draws the drop-shadow even though we have no NC area.
            if DwmIsCompositionEnabled().is_ok() {
                let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
                let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            }

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

        // Make the entire window rect the client area — removes the thin OS resize
        // border that WS_THICKFRAME would otherwise draw in the NC area.  The OS
        // still provides resize hit-testing via WM_NCHITTEST below.
        WM_NCCALCSIZE if wparam.0 != 0 => LRESULT(0),

        // Restore resize and drag behavior that WM_NCCALCSIZE=0 would otherwise lose.
        WM_NCHITTEST => {
            // Let the OS compute the default result first.
            let default = DefWindowProcW(hwnd, msg, wparam, lparam);

            // Client area: check manually for resize edges using cursor screen coords.
            if default == LRESULT(HTCLIENT as isize) {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

                let mut rc = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rc);

                let on_left  = x < rc.left   + RESIZE_BORDER;
                let on_right = x >= rc.right  - RESIZE_BORDER;
                let on_top   = y < rc.top     + RESIZE_BORDER;
                let on_bot   = y >= rc.bottom - RESIZE_BORDER;

                return match (on_left, on_right, on_top, on_bot) {
                    (true,  false, true,  false) => LRESULT(HTTOPLEFT    as isize),
                    (false, true,  true,  false) => LRESULT(HTTOPRIGHT   as isize),
                    (true,  false, false, true ) => LRESULT(HTBOTTOMLEFT as isize),
                    (false, true,  false, true ) => LRESULT(HTBOTTOMRIGHT as isize),
                    (true,  false, false, false) => LRESULT(HTLEFT        as isize),
                    (false, true,  false, false) => LRESULT(HTRIGHT       as isize),
                    (false, false, true,  false) => LRESULT(HTTOP         as isize),
                    (false, false, false, true ) => LRESULT(HTBOTTOM      as isize),
                    _ => {
                        // Inside the custom 36px titlebar → allow dragging.
                        // The titlebar buttons are handled in client-area mouse events.
                        let client_y = y - rc.top;
                        if client_y < 36 {
                            LRESULT(HTCAPTION as isize)
                        } else {
                            LRESULT(HTCLIENT as isize)
                        }
                    }
                };
            }
            default
        }

        WM_SIZE => {
            let width = (lparam.0 & 0xFFFF) as u32;
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
            let vk = wparam.0 as u32;
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
    if ptr == 0 {
        return;
    }
    let state = &*(ptr as *const WindowState);
    let _ = state.tx.try_send(event);
}
