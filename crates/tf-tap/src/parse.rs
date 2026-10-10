use crate::{BlockedKind, Mark, ProgramState, ProgramStatusReport, ProgressState, TapEvent};

/// Parse a complete OSC payload (the bytes between `ESC ]` and the
/// terminator). Returns `None` for sequences TermForge does not handle.
pub fn parse_osc(payload: &[u8]) -> Option<TapEvent> {
    let text = std::str::from_utf8(payload).ok()?;
    let (code, rest) = match text.split_once(';') {
        Some((c, r)) => (c, Some(r)),
        None => (text, None),
    };
    match code {
        "133" => parse_133(rest?),
        "7" => parse_osc7(rest?).map(TapEvent::Cwd),
        "9" => parse_osc9(rest?),
        "777" => parse_777(rest?),
        "99" => parse_99(rest?),
        "7501" => parse_7501(rest?, payload.len()),
        _ => None,
    }
}

fn parse_7501(rest: &str, payload_len: usize) -> Option<TapEvent> {
    // OSC (2 bytes) + payload + ST (2 bytes) must fit the protocol's cap.
    if payload_len > 4092 {
        return None;
    }
    if rest == "?" {
        return Some(TapEvent::ProgramStatusQuery);
    }

    let mut state = None;
    let mut id = None;
    let mut id_seen = false;
    let mut kind = None;
    let mut progress = None;
    let mut app = None;
    let mut title = None;
    let mut message = None;

    for pair in rest.split(':') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_lowercase())
            || key.len() > 16
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
        {
            continue;
        }
        match key {
            "state" => {
                state = match value {
                    "idle" => Some(ProgramState::Idle),
                    "working" => Some(ProgramState::Working),
                    "done" => Some(ProgramState::Done),
                    "blocked" => Some(ProgramState::Blocked),
                    "error" => Some(ProgramState::Error),
                    "clear" => Some(ProgramState::Clear),
                    _ => return None,
                }
            }
            "id" => {
                id_seen = true;
                id = valid_id(value).then(|| value.to_owned());
            }
            "kind" => {
                kind = match value {
                    "permission" => Some(BlockedKind::Permission),
                    "question" => Some(BlockedKind::Question),
                    "auth" => Some(BlockedKind::Auth),
                    _ => None,
                }
            }
            "progress" => progress = value.parse::<u8>().ok().filter(|p| *p <= 100),
            "app" => app = valid_name(value).then(|| value.to_owned()),
            "title" => title = Some(decode_text(value, 256, 192)?),
            "msg" => message = Some(decode_text(value, 2732, 2048)?),
            _ => {}
        }
    }

    let state = state?;
    if id_seen && id.is_none() {
        return None;
    }
    if state != ProgramState::Blocked {
        kind = None;
    }
    if !matches!(state, ProgramState::Working | ProgramState::Blocked) {
        progress = None;
    }
    Some(TapEvent::ProgramStatus(ProgramStatusReport {
        state,
        id,
        kind,
        progress,
        app,
        title,
        message,
    }))
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.split('/').count() <= 8
        && value.split('/').all(valid_name)
}

fn decode_text(value: &str, encoded_limit: usize, decoded_limit: usize) -> Option<String> {
    if value.len() > encoded_limit {
        return None;
    }
    let bytes = decode_base64(value)?;
    if bytes.len() > decoded_limit {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let padding = input.bytes().rev().take_while(|byte| *byte == b'=').count();
    if padding > 2
        || input[..input.len().saturating_sub(padding)].contains('=')
        || input.len().saturating_sub(padding) % 4 == 1
        || (padding > 0
            && (!input.len().is_multiple_of(4) || padding != (4 - (input.len() - padding) % 4) % 4))
    {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut bits = 0u32;
    let mut bit_count = 0u8;
    let mut padding = false;
    for byte in input.bytes() {
        if byte == b'=' {
            padding = true;
            continue;
        }
        if padding {
            return None;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
            bits &= (1 << bit_count) - 1;
        }
    }
    if bit_count > 0 && bits != 0 {
        return None;
    }
    Some(out)
}

fn parse_133(rest: &str) -> Option<TapEvent> {
    let mut parts = rest.split(';');
    let kind = parts.next()?;
    let mark = match kind {
        "A" => Mark::PromptStart,
        "B" => Mark::InputStart,
        "C" => Mark::CommandExecuted,
        "D" => {
            let exit_code = parts.next().and_then(|s| s.trim().parse::<i32>().ok());
            Mark::CommandFinished { exit_code }
        }
        _ => return None,
    };
    Some(TapEvent::Mark(mark))
}

/// `OSC 7 ; file://hostname/path` with percent-encoding.
fn parse_osc7(rest: &str) -> Option<crate::WorkingDirectory> {
    let after_scheme = rest.strip_prefix("file://")?;
    let path_start = after_scheme.find('/')?;
    let decoded = percent_decode(&after_scheme[path_start..])?;
    let host = &after_scheme[..path_start];
    Some(crate::WorkingDirectory {
        host: (!host.is_empty()).then(|| host.to_owned()),
        path: normalize_path(decoded),
    })
}

fn parse_osc9(rest: &str) -> Option<TapEvent> {
    if let Some(path) = rest.strip_prefix("9;") {
        let path = path.trim_matches('"');
        if path.is_empty() {
            return None;
        }
        return Some(TapEvent::Cwd(crate::WorkingDirectory {
            host: None,
            path: normalize_path(path.to_owned()),
        }));
    }
    if let Some(progress) = rest.strip_prefix("4;") {
        let mut parts = progress.split(';');
        let state = match parts.next()? {
            "0" => ProgressState::Clear,
            "1" => ProgressState::Normal,
            "2" => ProgressState::Error,
            "3" => ProgressState::Indeterminate,
            "4" => ProgressState::Paused,
            _ => return None,
        };
        let percent = parts
            .next()
            .and_then(|s| s.parse::<u8>().ok())
            .map(|p| p.min(100));
        return Some(TapEvent::Progress { state, percent });
    }
    // Other ConEmu sub-commands are numeric (`9;1;...` .. `9;12`); ignore them.
    let first = rest.split(';').next().unwrap_or_default();
    if !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if rest.is_empty() {
        return None;
    }
    Some(TapEvent::Notify {
        title: None,
        body: rest.to_owned(),
    })
}

fn parse_777(rest: &str) -> Option<TapEvent> {
    let rest = rest.strip_prefix("notify;")?;
    let (title, body) = rest.split_once(';').unwrap_or((rest, ""));
    let title = (!title.is_empty()).then(|| title.to_owned());
    Some(TapEvent::Notify {
        title,
        body: body.to_owned(),
    })
}

/// Minimal kitty `OSC 99` support: single-chunk notifications where the
/// payload is plain text. Multi-chunk (`d=0`) and base64 (`e=1`) payloads
/// are ignored for now.
fn parse_99(rest: &str) -> Option<TapEvent> {
    let (meta, payload) = rest.split_once(';')?;
    let mut is_title = false;
    for kv in meta.split(':').filter(|s| !s.is_empty()) {
        match kv.split_once('=') {
            Some(("d", "0")) | Some(("e", "1")) => return None,
            Some(("p", "title")) => is_title = true,
            Some(("p", "body")) => is_title = false,
            Some(("p", _)) => return None,
            _ => {}
        }
    }
    if payload.is_empty() {
        return None;
    }
    Some(if is_title {
        TapEvent::Notify {
            title: Some(payload.to_owned()),
            body: String::new(),
        }
    } else {
        TapEvent::Notify {
            title: None,
            body: payload.to_owned(),
        }
    })
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `/C:/Users/x` -> `C:/Users/x`. POSIX paths are returned unchanged.
fn normalize_path(path: String) -> String {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        path[1..].to_owned()
    } else {
        path
    }
}
