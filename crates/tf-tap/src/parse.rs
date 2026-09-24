use crate::{Mark, ProgressState, TapEvent};

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
        _ => None,
    }
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
fn parse_osc7(rest: &str) -> Option<String> {
    let after_scheme = rest.strip_prefix("file://")?;
    let path_start = after_scheme.find('/')?;
    let decoded = percent_decode(&after_scheme[path_start..])?;
    Some(normalize_path(decoded))
}

fn parse_osc9(rest: &str) -> Option<TapEvent> {
    if let Some(path) = rest.strip_prefix("9;") {
        let path = path.trim_matches('"');
        if path.is_empty() {
            return None;
        }
        return Some(TapEvent::Cwd(normalize_path(path.to_owned())));
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
