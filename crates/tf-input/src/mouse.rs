//! Mouse reporting (xterm X10/normal and SGR 1006 encodings).
//!
//! The UI decides *whether* to report (the program enabled tracking and the
//! user is not holding Shift to select); this module only produces bytes.

use crate::Mods;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press(MouseButton),
    Release(MouseButton),
    /// Motion, with the button held (if any).
    Motion(Option<MouseButton>),
}

/// Which events the program asked for; mirrors `tf_engine::MouseTracking`
/// without depending on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tracking {
    #[default]
    Off,
    Click,
    Drag,
    Motion,
}

/// Encode a mouse event at 0-based cell `(col, row)`, or `None` when the
/// current tracking mode does not want it.
pub fn encode_mouse(
    action: MouseAction,
    col: u16,
    row: u16,
    mods: Mods,
    tracking: Tracking,
    sgr: bool,
) -> Option<Vec<u8>> {
    let wants = match (tracking, action) {
        (Tracking::Off, _) => false,
        (_, MouseAction::Press(_) | MouseAction::Release(_)) => true,
        (Tracking::Drag, MouseAction::Motion(held)) => held.is_some(),
        (Tracking::Motion, MouseAction::Motion(_)) => true,
        (Tracking::Click, MouseAction::Motion(_)) => false,
    };
    // Wheel "releases" do not exist.
    if !wants
        || matches!(
            action,
            MouseAction::Release(MouseButton::WheelUp | MouseButton::WheelDown)
        )
    {
        return None;
    }
    let button_code = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    let mut code: u32 = match action {
        MouseAction::Press(b) => button_code(b),
        // Legacy encoding cannot say which button was released.
        MouseAction::Release(b) => {
            if sgr {
                button_code(b)
            } else {
                3
            }
        }
        MouseAction::Motion(held) => 32 + held.map_or(3, button_code),
    };
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.ctrl {
        code += 16;
    }
    let (x, y) = (u32::from(col) + 1, u32::from(row) + 1);
    if sgr {
        let end = if matches!(action, MouseAction::Release(_)) {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    // X10-style bytes only reach column/row 223.
    if x > 223 || y > 223 {
        return None;
    }
    Some(vec![
        0x1b,
        b'[',
        b'M',
        (32 + code) as u8,
        (32 + x) as u8,
        (32 + y) as u8,
    ])
}

/// Focus in/out report (DECSET 1004).
pub fn encode_focus(focused: bool) -> &'static [u8] {
    if focused {
        b"\x1b[I"
    } else {
        b"\x1b[O"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: Mods = Mods::NONE;

    #[test]
    fn off_reports_nothing() {
        let a = MouseAction::Press(MouseButton::Left);
        assert_eq!(encode_mouse(a, 0, 0, N, Tracking::Off, true), None);
    }

    #[test]
    fn sgr_press_and_release() {
        let p = encode_mouse(
            MouseAction::Press(MouseButton::Left),
            4,
            9,
            N,
            Tracking::Click,
            true,
        );
        assert_eq!(p.as_deref(), Some(&b"\x1b[<0;5;10M"[..]));
        let r = encode_mouse(
            MouseAction::Release(MouseButton::Right),
            0,
            0,
            N,
            Tracking::Click,
            true,
        );
        assert_eq!(r.as_deref(), Some(&b"\x1b[<2;1;1m"[..]));
    }

    #[test]
    fn modifiers_and_wheel() {
        let ctrl = Mods { ctrl: true, ..N };
        let w = encode_mouse(
            MouseAction::Press(MouseButton::WheelUp),
            0,
            0,
            ctrl,
            Tracking::Click,
            true,
        );
        assert_eq!(w.as_deref(), Some(&b"\x1b[<80;1;1M"[..]));
        let wr = encode_mouse(
            MouseAction::Release(MouseButton::WheelUp),
            0,
            0,
            N,
            Tracking::Click,
            true,
        );
        assert_eq!(wr, None);
    }

    #[test]
    fn motion_depends_on_mode() {
        let held = MouseAction::Motion(Some(MouseButton::Left));
        let free = MouseAction::Motion(None);
        assert_eq!(encode_mouse(held, 1, 1, N, Tracking::Click, true), None);
        assert_eq!(
            encode_mouse(held, 1, 1, N, Tracking::Drag, true).as_deref(),
            Some(&b"\x1b[<32;2;2M"[..])
        );
        assert_eq!(encode_mouse(free, 1, 1, N, Tracking::Drag, true), None);
        assert_eq!(
            encode_mouse(free, 1, 1, N, Tracking::Motion, true).as_deref(),
            Some(&b"\x1b[<35;2;2M"[..])
        );
    }

    #[test]
    fn legacy_encoding_and_its_limit() {
        let p = encode_mouse(
            MouseAction::Press(MouseButton::Left),
            0,
            0,
            N,
            Tracking::Click,
            false,
        );
        assert_eq!(p, Some(vec![0x1b, b'[', b'M', 32, 33, 33]));
        let r = encode_mouse(
            MouseAction::Release(MouseButton::Left),
            0,
            0,
            N,
            Tracking::Click,
            false,
        );
        assert_eq!(r, Some(vec![0x1b, b'[', b'M', 35, 33, 33]));
        let far = encode_mouse(
            MouseAction::Press(MouseButton::Left),
            300,
            0,
            N,
            Tracking::Click,
            false,
        );
        assert_eq!(far, None);
    }
}
