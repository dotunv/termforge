/// Maps a Win32 virtual-key code to the ANSI/VT byte sequence to write to the PTY.
/// Returns `None` for keys that should produce no PTY output (e.g. modifiers alone).
pub fn vk_to_pty_bytes(vk: u32, ctrl: bool) -> Option<Vec<u8>> {
    // Ctrl+letter → control character
    if ctrl {
        let ch = match vk {
            0x41..=0x5A => (vk - 0x40) as u8, // Ctrl+A = 0x01 … Ctrl+Z = 0x1A
            _ => return None,
        };
        return Some(vec![ch]);
    }

    let seq: &[u8] = match vk {
        VK_RETURN   => b"\r",
        VK_BACK     => b"\x7f",
        VK_TAB      => b"\t",
        VK_ESCAPE   => b"\x1b",
        VK_UP       => b"\x1b[A",
        VK_DOWN     => b"\x1b[B",
        VK_RIGHT    => b"\x1b[C",
        VK_LEFT     => b"\x1b[D",
        VK_HOME     => b"\x1b[H",
        VK_END      => b"\x1b[F",
        VK_INSERT   => b"\x1b[2~",
        VK_DELETE   => b"\x1b[3~",
        VK_PRIOR    => b"\x1b[5~", // Page Up
        VK_NEXT     => b"\x1b[6~", // Page Down
        VK_F1       => b"\x1bOP",
        VK_F2       => b"\x1bOQ",
        VK_F3       => b"\x1bOR",
        VK_F4       => b"\x1bOS",
        VK_F5       => b"\x1b[15~",
        VK_F6       => b"\x1b[17~",
        VK_F7       => b"\x1b[18~",
        VK_F8       => b"\x1b[19~",
        VK_F9       => b"\x1b[20~",
        VK_F10      => b"\x1b[21~",
        VK_F11      => b"\x1b[23~",
        VK_F12      => b"\x1b[24~",
        _ => return None,
    };
    Some(seq.to_vec())
}

/// Convert a WM_CHAR UTF-16 code unit to UTF-8 bytes for the PTY.
/// Control chars (< 0x20) pass through directly.
pub fn char_to_pty_bytes(code_unit: u16) -> Option<Vec<u8>> {
    if code_unit == 0 { return None; }
    // Surrogate halves are incomplete — skip them for now (Phase 3: buffer pairs).
    if (0xD800..=0xDFFF).contains(&code_unit) { return None; }
    // Control chars pass through as raw bytes.
    if code_unit < 0x20 {
        return Some(vec![code_unit as u8]);
    }
    let ch = char::from_u32(code_unit as u32)?;
    let mut buf = [0u8; 4];
    Some(ch.encode_utf8(&mut buf).as_bytes().to_vec())
}

// Win32 virtual-key constants (subset we need)
const VK_RETURN : u32 = 0x0D;
const VK_BACK   : u32 = 0x08;
const VK_TAB    : u32 = 0x09;
const VK_ESCAPE : u32 = 0x1B;
const VK_PRIOR  : u32 = 0x21;
const VK_NEXT   : u32 = 0x22;
const VK_END    : u32 = 0x23;
const VK_HOME   : u32 = 0x24;
const VK_LEFT   : u32 = 0x25;
const VK_UP     : u32 = 0x26;
const VK_RIGHT  : u32 = 0x27;
const VK_DOWN   : u32 = 0x28;
const VK_INSERT : u32 = 0x2D;
const VK_DELETE : u32 = 0x2E;
const VK_F1     : u32 = 0x70;
const VK_F2     : u32 = 0x71;
const VK_F3     : u32 = 0x72;
const VK_F4     : u32 = 0x73;
const VK_F5     : u32 = 0x74;
const VK_F6     : u32 = 0x75;
const VK_F7     : u32 = 0x76;
const VK_F8     : u32 = 0x77;
const VK_F9     : u32 = 0x78;
const VK_F10    : u32 = 0x79;
const VK_F11    : u32 = 0x7A;
const VK_F12    : u32 = 0x7B;
