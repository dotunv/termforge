//! End-to-end tests of the processing pipeline.

use tf_engine::{AlacrittyEngine, GridSize, TerminalEngine};
use tf_session::{BlockState, Processor, SessionEvent};

#[test]
fn synthetic_shell_output_produces_anchored_blocks() {
    let mut p = Processor::new(AlacrittyEngine::new(GridSize::new(40, 10)));
    let mut ev = Vec::new();
    let stream: &[u8] = b"\x1b]9;9;\"C:\\src\"\x07\x1b]133;A\x07PS C:\\src> \x1b]133;B\x07dir\r\n\x1b]133;C\x07a.txt\r\nb.txt\r\n\x1b]133;D;0\x07\x1b]133;A\x07PS C:\\src> \x1b]133;B\x07";
    // Feed one byte at a time to prove chunking does not matter.
    for b in stream {
        p.process(std::slice::from_ref(b), &mut ev);
    }
    assert!(ev.contains(&SessionEvent::Cwd("C:\\src".into())));
    assert!(ev.contains(&SessionEvent::BlockFinished {
        index: 0,
        exit_code: Some(0)
    }));

    let first = p.blocks().get(0).unwrap();
    assert_eq!(first.state, BlockState::Finished);
    assert_eq!(first.prompt_line, 0);
    assert_eq!(first.output_line, Some(1));
    assert_eq!(first.end_line, Some(3));
    assert_eq!(first.cwd.as_deref(), Some("C:\\src"));
    assert_eq!(p.blocks().get(1).unwrap().state, BlockState::Prompt);
    assert!(p.engine().snapshot().text().contains("b.txt"));
}

/// Real shell, real PTY, real integration script. This is the seed of the
/// Phase 0 ConPTY ordering harness (run on Windows CI with PowerShell).
#[test]
#[cfg(unix)]
fn bash_integration_emits_ordered_marks() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use tf_pty::{discover_shells, Pty, PtyDims, ShellKind};

    let Some(bash) = discover_shells()
        .into_iter()
        .find(|s| s.kind == ShellKind::Bash)
    else {
        eprintln!("bash not found; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    tf_shell::install(dir.path()).unwrap();
    let profile = tf_shell::inject(&bash, dir.path()).unwrap();
    let home = tempfile::tempdir().unwrap();
    let env = [
        (
            "HOME".to_string(),
            home.path().to_string_lossy().into_owned(),
        ),
        ("PS1".to_string(), "$ ".to_string()),
    ];
    let (mut pty, mut reader) = Pty::spawn(
        &profile,
        Some(dir.path()),
        PtyDims { cols: 80, rows: 24 },
        &env,
    )
    .unwrap();

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = std::io::Read::read(&mut reader, &mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let mut p = Processor::new(AlacrittyEngine::new(GridSize::new(80, 24)));
    let mut events = Vec::new();
    let mut sent = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while p
        .blocks()
        .commands()
        .filter(|b| b.state == BlockState::Finished)
        .count()
        < 2
    {
        assert!(
            Instant::now() < deadline,
            "timed out; blocks = {:?}",
            p.blocks()
        );
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) {
            let reply = p.process(&chunk, &mut events);
            if !reply.is_empty() {
                pty.write_all(&reply).unwrap();
            }
        }
        if !sent && !p.blocks().is_empty() {
            pty.write_all(b"echo tf-one\r").unwrap();
            pty.write_all(b"false\r").unwrap();
            sent = true;
        }
    }
    let cmds: Vec<_> = p.blocks().commands().collect();
    assert_eq!(cmds[0].exit_code, Some(0));
    assert_eq!(cmds[1].exit_code, Some(1));
    for b in &cmds {
        let (a, c, d) = (b.prompt_line, b.output_line.unwrap(), b.end_line.unwrap());
        assert!(a <= c && c <= d, "marks out of order: {b:?}");
    }
    assert!(events.iter().any(|e| matches!(e, SessionEvent::Cwd(_))));
    let _ = pty.write_all(b"exit\r");
}
