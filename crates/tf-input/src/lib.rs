//! Translate key presses and pastes into the bytes a terminal program
//! expects, following xterm conventions.
//!
//! Framework-free: the UI maps its own key events onto [`Key`] and
//! [`Mods`], so the encoding rules are unit-tested here rather than buried
//! in view code.

/// A logical key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// Text produced by the key (already layout- and shift-resolved).
    Text(String),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// Function keys F1..=F12.
    F(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
    };

    /// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    fn param(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }

    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift
    }
}

/// Terminal modes that change how keys are encoded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputModes {
    /// DECCKM: cursor keys send `ESC O x` instead of `ESC [ x`.
    pub app_cursor: bool,
    /// Bracketed paste (mode 2004).
    pub bracketed_paste: bool,
}

impl Key {
    /// Map a platform key name (as used by GPUI, winit and the web
    /// `KeyboardEvent.key` spec, case-insensitive) to a non-text key.
    pub fn from_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        Some(match lower.as_str() {
            "enter" | "return" => Key::Enter,
            "tab" => Key::Tab,
            "backspace" => Key::Backspace,
            "escape" | "esc" => Key::Escape,
            "up" | "arrowup" => Key::Up,
            "down" | "arrowdown" => Key::Down,
            "left" | "arrowleft" => Key::Left,
            "right" | "arrowright" => Key::Right,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "insert" => Key::Insert,
            "delete" | "del" => Key::Delete,
            f if f.len() >= 2 && f.starts_with('f') => Key::F(f[1..].parse().ok()?),
            _ => return None,
        })
    }
}

/// Encode a key press. Returns `None` for combinations with no terminal
/// meaning, so the UI can use them for its own shortcuts.
pub fn encode_key(key: &Key, mods: Mods, modes: InputModes) -> Option<Vec<u8>> {
    let out = match key {
        Key::Text(text) => return encode_text(text, mods),
        Key::Enter => with_alt(b"\r", mods),
        Key::Tab if mods.shift && !mods.ctrl && !mods.alt => b"\x1b[Z".to_vec(),
        Key::Tab => with_alt(b"\t", mods),
        Key::Backspace if mods.ctrl => with_alt(b"\x08", mods),
        Key::Backspace => with_alt(b"\x7f", mods),
        Key::Escape => with_alt(b"\x1b", mods),
        Key::Up => cursor(b'A', mods, modes),
        Key::Down => cursor(b'B', mods, modes),
        Key::Right => cursor(b'C', mods, modes),
        Key::Left => cursor(b'D', mods, modes),
        Key::Home => cursor(b'H', mods, modes),
        Key::End => cursor(b'F', mods, modes),
        Key::Insert => tilde(2, mods),
        Key::Delete => tilde(3, mods),
        Key::PageUp => tilde(5, mods),
        Key::PageDown => tilde(6, mods),
        Key::F(n @ 1..=4) => {
            let c = b"PQRS"[usize::from(*n - 1)];
            if mods.any() {
                format!("\x1b[1;{}{}", mods.param(), c as char).into_bytes()
            } else {
                vec![0x1b, b'O', c]
            }
        }
        Key::F(n @ 5..=12) => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde(CODES[usize::from(*n - 5)], mods)
        }
        Key::F(_) => return None,
    };
    Some(out)
}

fn with_alt(bytes: &[u8], mods: Mods) -> Vec<u8> {
    let mut v = Vec::with_capacity(bytes.len() + 1);
    if mods.alt {
        v.push(0x1b);
    }
    v.extend_from_slice(bytes);
    v
}

fn cursor(c: u8, mods: Mods, modes: InputModes) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[1;{}{}", mods.param(), c as char).into_bytes()
    } else if modes.app_cursor {
        vec![0x1b, b'O', c]
    } else {
        vec![0x1b, b'[', c]
    }
}

fn tilde(code: u8, mods: Mods) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[{code};{}~", mods.param()).into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

fn encode_text(text: &str, mods: Mods) -> Option<Vec<u8>> {
    if text.is_empty() {
        return None;
    }
    // Ctrl+Alt is AltGr on many Windows layouts: the text is already the
    // intended character, so send it as-is.
    if mods.ctrl && mods.alt {
        return Some(text.as_bytes().to_vec());
    }
    if mods.ctrl {
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return None;
        };
        return ctrl_byte(c).map(|b| vec![b]);
    }
    Some(with_alt(text.as_bytes(), mods))
}

/// Control-key mapping (`Ctrl+A` -> 0x01, `Ctrl+[` -> ESC, `Ctrl+Space` -> NUL).
fn ctrl_byte(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '/' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

/// Encode pasted text.
///
/// Line endings become `\r` (what Enter sends). With bracketed paste on,
/// the text is wrapped in `ESC [200~ .. ESC [201~` and any embedded
/// terminator is removed so pasted content can never end the bracket early
/// and inject keystrokes.
pub fn encode_paste(text: &str, modes: InputModes) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if modes.bracketed_paste {
        // Loop: removing one marker can splice together a new one.
        let mut safe = normalized;
        while safe.contains("\x1b[201~") || safe.contains("\x1b[200~") {
            safe = safe.replace("\x1b[201~", "").replace("\x1b[200~", "");
        }
        let mut out = Vec::with_capacity(safe.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(safe.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        normalized.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn enc(key: Key, mods: Mods) -> Vec<u8> {
        encode_key(&key, mods, InputModes::default()).unwrap()
    }

    const CTRL: Mods = Mods {
        ctrl: true,
        alt: false,
        shift: false,
    };
    const ALT: Mods = Mods {
        ctrl: false,
        alt: true,
        shift: false,
    };
    const SHIFT: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: true,
    };

    #[test]
    fn text_and_control_keys() {
        assert_eq!(enc(Key::Text("é".into()), Mods::NONE), "é".as_bytes());
        assert_eq!(enc(Key::Text("c".into()), CTRL), b"\x03");
        assert_eq!(enc(Key::Text("C".into()), CTRL), b"\x03");
        assert_eq!(enc(Key::Text("[".into()), CTRL), b"\x1b");
        assert_eq!(enc(Key::Text(" ".into()), CTRL), b"\x00");
        assert_eq!(enc(Key::Text("b".into()), ALT), b"\x1bb");
        assert_eq!(
            encode_key(&Key::Text("é".into()), CTRL, InputModes::default()),
            None
        );
    }

    #[test]
    fn altgr_sends_the_character() {
        let altgr = Mods {
            ctrl: true,
            alt: true,
            shift: false,
        };
        assert_eq!(enc(Key::Text("@".into()), altgr), b"@");
    }

    #[test]
    fn editing_keys() {
        assert_eq!(enc(Key::Enter, Mods::NONE), b"\r");
        assert_eq!(enc(Key::Backspace, Mods::NONE), b"\x7f");
        assert_eq!(enc(Key::Backspace, CTRL), b"\x08");
        assert_eq!(enc(Key::Tab, SHIFT), b"\x1b[Z");
        assert_eq!(enc(Key::Delete, Mods::NONE), b"\x1b[3~");
        assert_eq!(enc(Key::PageUp, CTRL), b"\x1b[5;5~");
    }

    #[test]
    fn cursor_keys_respect_decckm_and_modifiers() {
        assert_eq!(enc(Key::Up, Mods::NONE), b"\x1b[A");
        let app = InputModes {
            app_cursor: true,
            ..Default::default()
        };
        assert_eq!(encode_key(&Key::Up, Mods::NONE, app).unwrap(), b"\x1bOA");
        assert_eq!(enc(Key::Right, CTRL), b"\x1b[1;5C");
        assert_eq!(encode_key(&Key::Left, SHIFT, app).unwrap(), b"\x1b[1;2D");
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc(Key::F(1), Mods::NONE), b"\x1bOP");
        assert_eq!(enc(Key::F(5), Mods::NONE), b"\x1b[15~");
        assert_eq!(enc(Key::F(12), SHIFT), b"\x1b[24;2~");
        assert_eq!(
            encode_key(&Key::F(13), Mods::NONE, InputModes::default()),
            None
        );
    }

    #[test]
    fn key_names() {
        assert_eq!(Key::from_name("enter"), Some(Key::Enter));
        assert_eq!(Key::from_name("ArrowUp"), Some(Key::Up));
        assert_eq!(Key::from_name("f11"), Some(Key::F(11)));
        assert_eq!(Key::from_name("f"), None);
        assert_eq!(Key::from_name("shift"), None);
        assert_eq!(Key::from_name("a"), None);
    }

    #[test]
    fn paste_normalizes_newlines() {
        assert_eq!(encode_paste("a\r\nb\nc", InputModes::default()), b"a\rb\rc");
    }

    #[test]
    fn bracketed_paste_cannot_be_escaped() {
        let modes = InputModes {
            bracketed_paste: true,
            ..Default::default()
        };
        let out = encode_paste("ls\x1b[201~rm -rf ~\n", modes);
        assert_eq!(out, b"\x1b[200~lsrm -rf ~\r\x1b[201~");
    }

    proptest! {
        #[test]
        fn bracketed_paste_has_exactly_one_terminator(s in ".*") {
            let modes = InputModes { bracketed_paste: true, ..Default::default() };
            let out = String::from_utf8(encode_paste(&s, modes)).unwrap();
            prop_assert!(out.starts_with("\x1b[200~") && out.ends_with("\x1b[201~"));
            prop_assert_eq!(out.matches("\x1b[201~").count(), 1);
            prop_assert_eq!(out.matches("\x1b[200~").count(), 1);
        }

        #[test]
        fn spliced_terminators_are_removed(prefix in "[a-z]{0,3}") {
            let modes = InputModes { bracketed_paste: true, ..Default::default() };
            let s = format!("{prefix}\x1b[20\x1b[201~1~");
            let out = String::from_utf8(encode_paste(&s, modes)).unwrap();
            prop_assert_eq!(out.matches("\x1b[201~").count(), 1);
        }
    }
}
