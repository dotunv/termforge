//! Small, dependency-light Language Server Protocol supervisor.
//!
//! One server follows the file currently open in the Files environment. The
//! process and its blocking stdio live off the GPUI thread; every event carries
//! a generation so output from a replaced or crashed server cannot leak into a
//! newer document.

use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub start: tf_editor::TextPosition,
    pub end: tf_editor::TextPosition,
    pub severity: Severity,
    pub source: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Debug)]
pub enum Event {
    Ready {
        generation: u64,
        server: String,
    },
    Diagnostics {
        generation: u64,
        path: PathBuf,
        version: Option<i64>,
        items: Vec<Diagnostic>,
    },
    Failed {
        generation: u64,
        message: String,
    },
    Stopped {
        generation: u64,
    },
}

#[derive(Debug)]
enum Instruction {
    Change { version: i64, text: String },
    Save,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerSpec {
    program: &'static str,
    args: &'static [&'static str],
    language_id: &'static str,
}

#[derive(Debug)]
pub struct Supervisor {
    generation: u64,
    active: Option<ActiveServer>,
    events_tx: mpsc::Sender<Event>,
    events_rx: mpsc::Receiver<Event>,
}

#[derive(Debug)]
struct ActiveServer {
    spec: ServerSpec,
    root: PathBuf,
    path: PathBuf,
    instructions: mpsc::Sender<Instruction>,
}

/// Always reap a server, including when framing or a pipe write fails.
struct ServerProcess(Child);

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        let (events_tx, events_rx) = mpsc::channel();
        Self {
            generation: 0,
            active: None,
            events_tx,
            events_rx,
        }
    }
}

impl Supervisor {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn open(&mut self, root: &Path, path: &Path, text: &str, revision: u64) -> bool {
        let Some(spec) = server_for_path(path) else {
            self.stop();
            return false;
        };
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.spec == spec && active.root == root && active.path == path)
        {
            self.change(text, revision);
            return true;
        }
        self.stop();
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        let (instructions, rx) = mpsc::channel();
        self.active = Some(ActiveServer {
            spec: spec.clone(),
            root: root.to_path_buf(),
            path: path.to_path_buf(),
            instructions,
        });
        let events = self.events_tx.clone();
        let root = root.to_path_buf();
        let path = path.to_path_buf();
        let text = text.to_owned();
        thread::Builder::new()
            .name(format!("termforge-lsp-{}", spec.language_id))
            .spawn(move || run_server(spec, root, path, text, revision, generation, rx, events))
            .expect("spawn LSP supervisor thread");
        true
    }

    pub fn change(&mut self, text: &str, revision: u64) {
        let Some(active) = &self.active else { return };
        let version = i64::try_from(revision).unwrap_or(i64::MAX);
        if active
            .instructions
            .send(Instruction::Change {
                version,
                text: text.to_owned(),
            })
            .is_err()
        {
            self.active = None;
        }
    }

    pub fn save(&mut self) {
        if let Some(active) = &self.active {
            if active.instructions.send(Instruction::Save).is_err() {
                self.active = None;
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(active) = self.active.take() {
            self.generation = self.generation.saturating_add(1);
            let _ = active.instructions.send(Instruction::Stop);
        }
    }

    pub fn acknowledge_stopped(&mut self, generation: u64) {
        if generation == self.generation {
            self.active = None;
        }
    }

    pub fn drain(&mut self) -> Vec<Event> {
        self.events_rx.try_iter().collect()
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn server_for_path(path: &Path) -> Option<ServerSpec> {
    let extension = path.extension().and_then(|value| value.to_str())?;
    let candidates: &[ServerSpec] = match extension {
        "rs" => &[ServerSpec {
            program: "rust-analyzer",
            args: &[],
            language_id: "rust",
        }],
        "ts" | "tsx" | "js" | "jsx" => &[ServerSpec {
            program: "typescript-language-server",
            args: &["--stdio"],
            language_id: match extension {
                "tsx" => "typescriptreact",
                "ts" => "typescript",
                "jsx" => "javascriptreact",
                _ => "javascript",
            },
        }],
        "py" => &[ServerSpec {
            program: "pylsp",
            args: &[],
            language_id: "python",
        }],
        "go" => &[ServerSpec {
            program: "gopls",
            args: &[],
            language_id: "go",
        }],
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" => &[ServerSpec {
            program: "clangd",
            args: &[],
            language_id: if matches!(extension, "c" | "h") {
                "c"
            } else {
                "cpp"
            },
        }],
        _ => &[],
    };
    candidates
        .iter()
        .find(|candidate| crate::surface::on_path(candidate.program))
        .cloned()
}

#[allow(clippy::too_many_arguments)]
fn run_server(
    spec: ServerSpec,
    root: PathBuf,
    path: PathBuf,
    text: String,
    revision: u64,
    generation: u64,
    instructions: mpsc::Receiver<Instruction>,
    events: mpsc::Sender<Event>,
) {
    if let Err(error) = run_server_inner(
        &spec,
        &root,
        &path,
        &text,
        revision,
        generation,
        &instructions,
        &events,
    ) {
        let _ = events.send(Event::Failed {
            generation,
            message: format!("{}: {error}", spec.program),
        });
    }
    let _ = events.send(Event::Stopped { generation });
}

#[allow(clippy::too_many_arguments)]
fn run_server_inner(
    spec: &ServerSpec,
    root: &Path,
    path: &Path,
    text: &str,
    revision: u64,
    generation: u64,
    instructions: &mpsc::Receiver<Instruction>,
    events: &mpsc::Sender<Event>,
) -> io::Result<()> {
    let mut child = ServerProcess(
        Command::new(spec.program)
            .args(spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let stdin = Arc::new(Mutex::new(
        child
            .0
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("missing stdin"))?,
    ));
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing stdout"))?;
    let root_uri = file_uri(root);
    write_message(
        &stdin,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {"textDocument": {"publishDiagnostics": {"versionSupport": true}}},
                "clientInfo": {"name": "TermForge", "version": env!("CARGO_PKG_VERSION")}
            }
        }),
    )?;
    let reader_stdin = Arc::clone(&stdin);
    let reader_events = events.clone();
    let (reader_done_tx, reader_done_rx) = mpsc::channel();
    let (initialized_tx, initialized_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Ok(message) = read_message(&mut reader) {
            if message.get("id").and_then(Value::as_i64) == Some(1) {
                let outcome = message
                    .get("error")
                    .map_or_else(|| Ok(()), |error| Err(error.to_string()));
                let _ = initialized_tx.send(outcome);
            } else if let Some((path, version, items)) = parse_diagnostics(&message) {
                let _ = reader_events.send(Event::Diagnostics {
                    generation,
                    path,
                    version,
                    items,
                });
            } else if message.get("method").is_some() {
                if let Some(id) = message.get("id").cloned() {
                    let _ = write_message(
                        &reader_stdin,
                        &json!({"jsonrpc":"2.0","id":id,"result":null}),
                    );
                }
            }
        }
        let _ = reader_done_tx.send(());
    });
    match initialized_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            return Err(io::Error::other(format!("initialize failed: {error}")));
        }
        Err(error) => {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("initialize timed out: {error}"),
            ));
        }
    }
    write_message(
        &stdin,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )?;
    let uri = file_uri(path);
    write_message(
        &stdin,
        &json!({
            "jsonrpc":"2.0", "method":"textDocument/didOpen", "params":{"textDocument":{
                "uri":uri, "languageId":spec.language_id,
                "version":i64::try_from(revision).unwrap_or(i64::MAX), "text":text
            }}
        }),
    )?;
    let _ = events.send(Event::Ready {
        generation,
        server: spec.program.into(),
    });

    loop {
        if reader_done_rx.try_recv().is_ok() || child.0.try_wait()?.is_some() {
            return Err(io::Error::other("language server exited unexpectedly"));
        }
        match instructions.recv_timeout(Duration::from_millis(200)) {
            Ok(Instruction::Change { version, text }) => write_message(
                &stdin,
                &json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{
                    "textDocument":{"uri":uri,"version":version},
                    "contentChanges":[{"text":text}]
                }}),
            )?,
            Ok(Instruction::Save) => write_message(
                &stdin,
                &json!({"jsonrpc":"2.0","method":"textDocument/didSave","params":{"textDocument":{"uri":uri}}}),
            )?,
            Ok(Instruction::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    let _ = write_message(
        &stdin,
        &json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}),
    );
    let _ = write_message(
        &stdin,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    );
    for _ in 0..5 {
        if child.0.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn write_message(stdin: &Arc<Mutex<ChildStdin>>, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    let mut stdin = stdin
        .lock()
        .map_err(|_| io::Error::other("language server stdin lock poisoned"))?;
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len())?;
    stdin.write_all(&body)?;
    stdin.flush()
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Value> {
    let mut length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "language server closed stdout",
            ));
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some(value) = header.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    if length > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "language server message too large",
        ));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}

fn parse_diagnostics(message: &Value) -> Option<(PathBuf, Option<i64>, Vec<Diagnostic>)> {
    if message.get("method")?.as_str()? != "textDocument/publishDiagnostics" {
        return None;
    }
    let params = message.get("params")?;
    let path = path_from_file_uri(params.get("uri")?.as_str()?)?;
    let diagnostics = params.get("diagnostics")?.as_array()?;
    Some((
        path.clone(),
        params.get("version").and_then(Value::as_i64),
        diagnostics
            .iter()
            .filter_map(|item| {
                let range = item.get("range")?;
                Some(Diagnostic {
                    path: path.clone(),
                    start: parse_position(range.get("start")?)?,
                    end: parse_position(range.get("end")?)?,
                    severity: match item.get("severity").and_then(Value::as_u64).unwrap_or(3) {
                        1 => Severity::Error,
                        2 => Severity::Warning,
                        4 => Severity::Hint,
                        _ => Severity::Information,
                    },
                    source: item
                        .get("source")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    message: item.get("message")?.as_str()?.to_owned(),
                })
            })
            .collect(),
    ))
}

fn parse_position(value: &Value) -> Option<tf_editor::TextPosition> {
    Some(tf_editor::TextPosition {
        line: u32::try_from(value.get("line")?.as_u64()?).ok()?,
        utf16_column: u32::try_from(value.get("character")?.as_u64()?).ok()?,
    })
}

fn file_uri(path: &Path) -> String {
    let path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let display = path.to_string_lossy().replace('\\', "/");
    let prefix = if display.starts_with('/') {
        "file://"
    } else {
        "file:///"
    };
    format!("{prefix}{}", percent_encode(&display))
}

fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let decoded = percent_decode(encoded)?;
    #[cfg(windows)]
    let decoded = decoded.strip_prefix('/').unwrap_or(&decoded).to_owned();
    Some(PathBuf::from(decoded))
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            output.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_is_bounded_and_round_trips() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let framed = format!("Content-Length: {}\r\nX-Test: yes\r\n\r\n", body.len())
            .into_bytes()
            .into_iter()
            .chain(body.iter().copied())
            .collect::<Vec<_>>();
        let value = read_message(&mut BufReader::new(framed.as_slice())).unwrap();
        assert_eq!(value["id"], 1);
    }

    #[test]
    fn diagnostics_parse_utf16_positions_and_severity() {
        let message = json!({"method":"textDocument/publishDiagnostics","params":{
            "uri":"file:///tmp/a%20b.rs", "version":7, "diagnostics":[{"range":{
                "start":{"line":2,"character":4},"end":{"line":2,"character":6}},
                "severity":1,"source":"rustc","message":"bad type"}]
        }});
        let (path, version, diagnostics) = parse_diagnostics(&message).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/a b.rs"));
        assert_eq!(version, Some(7));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].path, PathBuf::from("/tmp/a b.rs"));
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].start.utf16_column, 4);
    }

    #[test]
    fn empty_diagnostic_publication_retains_its_file_identity() {
        let message = json!({"method":"textDocument/publishDiagnostics","params":{
            "uri":"file:///tmp/cleared.rs", "diagnostics":[]
        }});
        let (path, version, diagnostics) = parse_diagnostics(&message).unwrap();
        assert!(path.ends_with("cleared.rs"));
        assert_eq!(version, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn file_uris_escape_spaces_and_unicode() {
        let encoded = percent_encode("/tmp/a b/β.rs");
        assert_eq!(percent_decode(&encoded).as_deref(), Some("/tmp/a b/β.rs"));
        assert!(encoded.contains("%20") && encoded.contains("%CE%B2"));
    }

    #[test]
    fn framing_rejects_oversized_messages_before_allocating_body() {
        let framed = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);
        let error = read_message(&mut BufReader::new(framed.as_bytes())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_exchanges_messages_and_stops_a_child() {
        const SCRIPT: &str = r#"
            init='{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}'
            diag='{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///tmp/test.rs","version":0,"diagnostics":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"severity":1,"message":"test error"}]}}'
            printf 'Content-Length: %s\r\n\r\n%s' "${#init}" "$init"
            printf 'Content-Length: %s\r\n\r\n%s' "${#diag}" "$diag"
            cat >/dev/null
        "#;
        let spec = ServerSpec {
            program: "sh",
            args: &["-c", SCRIPT],
            language_id: "rust",
        };
        let (instructions_tx, instructions_rx) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            run_server(
                spec,
                PathBuf::from("/tmp"),
                PathBuf::from("/tmp/test.rs"),
                "x".into(),
                0,
                9,
                instructions_rx,
                events_tx,
            );
        });
        let mut ready = false;
        let mut diagnostic = false;
        for _ in 0..2 {
            match events_rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::Ready { generation: 9, .. } => ready = true,
                Event::Diagnostics {
                    generation: 9,
                    version: Some(0),
                    items,
                    ..
                } => diagnostic = items.len() == 1 && items[0].message == "test error",
                event => panic!("unexpected server event: {event:?}"),
            }
        }
        assert!(ready && diagnostic);
        instructions_tx.send(Instruction::Stop).unwrap();
        assert!(matches!(
            events_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Stopped { generation: 9 }
        ));
        worker.join().unwrap();
    }
}
