#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns
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

#[test]
fn clearing_scrollback_prunes_stale_blocks() {
    let mut p = Processor::new(AlacrittyEngine::new(GridSize::new(40, 5)));
    let mut ev = Vec::new();
    for i in 0..6 {
        let cmd = format!(
            "\x1b]133;A\x07$ \x1b]133;B\x07c{i}\r\n\x1b]133;C\x07out{i}\r\n\x1b]133;D;0\x07"
        );
        p.process(cmd.as_bytes(), &mut ev);
    }
    assert_eq!(p.blocks().commands().count(), 6);
    // What `clear` sends: home, erase screen, erase scrollback.
    p.process(b"\x1b]133;A\x07$ \x1b]133;B\x07clear\r\n\x1b]133;C\x07\x1b[H\x1b[2J\x1b[3J\x1b]133;D;0\x07\x1b]133;A\x07$ ", &mut ev);
    let top = p.engine().snapshot().top_line_abs();
    for b in p.blocks().iter() {
        assert!(b.end_line.is_none_or(|e| e >= top), "stale block {b:?}");
    }
    assert_eq!(p.blocks().commands().count(), 0, "{:?}", p.blocks());
}

#[test]
fn alternate_screen_does_not_move_blocks() {
    let mut p = Processor::new(AlacrittyEngine::new(GridSize::new(40, 5)));
    let mut ev = Vec::new();
    let mut run = |cmd: &str, out: &str| {
        let s =
            format!("\x1b]133;A\x07$ \x1b]133;B\x07{cmd}\r\n\x1b]133;C\x07{out}\x1b]133;D;0\x07");
        p.process(s.as_bytes(), &mut ev);
    };
    run("seq", &"n\r\n".repeat(20));
    run("vim", "\x1b[?1049hfull screen\x1b[?1049l");
    p.process(b"\x1b]133;A\x07$ ", &mut ev);
    let lines: Vec<_> = p.blocks().iter().map(|b| b.prompt_line).collect();
    assert_eq!(lines, vec![0, 21, 22], "{:?}", p.blocks());
}

/// Real shell, real PTY, real integration script: the ConPTY ordering
/// harness from the build plan. Each command must produce A <= C <= D marks
/// in order with the right exit code.
fn assert_integration(kind: tf_pty::ShellKind, commands: &[&str], codes: &[i32]) {
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};
    use tf_session::{LiveSession, SpawnOptions};

    let Some(shell) = tf_pty::discover_shells()
        .into_iter()
        .find(|s| s.kind == kind)
    else {
        eprintln!("{kind:?} not found; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (tx, rx) = mpsc::channel::<()>();
    let tx = Mutex::new(tx);
    let session = LiveSession::spawn(
        SpawnOptions {
            profile: shell,
            cwd: Some(dir.path().to_path_buf()),
            size: GridSize::new(100, 30),
            env: vec![
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                ("PS1".into(), "$ ".into()),
            ],
            integration_dir: Some(dir.path().join("shell")),
        },
        Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        }),
    )
    .unwrap();

    let finished = |s: &LiveSession| {
        s.with(|p| {
            p.blocks()
                .commands()
                .filter(|b| b.state == BlockState::Finished)
                .count()
        })
    };
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut sent = 0;
    while finished(&session) < commands.len() {
        assert!(
            Instant::now() < deadline,
            "timed out; blocks = {:?}\nscreen:\n{}",
            session.with(|p| format!("{:?}", p.blocks())),
            session.snapshot().text()
        );
        let _ = rx.recv_timeout(Duration::from_millis(200));
        // Send the next command once the shell is sitting at a prompt.
        let at_prompt = session.with(|p| {
            p.blocks()
                .iter()
                .last()
                .is_some_and(|b| b.state == BlockState::Prompt && b.input_line.is_some())
        });
        if at_prompt && sent < commands.len() && finished(&session) == sent {
            session
                .write(format!("{}\r", commands[sent]).as_bytes())
                .unwrap();
            sent += 1;
        }
    }

    let cmds: Vec<_> = session.with(|p| p.blocks().commands().cloned().collect());
    for (b, code) in cmds.iter().zip(codes) {
        assert_eq!(b.exit_code, Some(*code), "block {b:?}");
        let (a, c, d) = (b.prompt_line, b.output_line.unwrap(), b.end_line.unwrap());
        assert!(a <= c && c <= d, "marks out of order: {b:?}");
    }
    assert!(session
        .take_events()
        .iter()
        .any(|e| matches!(e, SessionEvent::Cwd(_))));
    let _ = session.write(b"exit\r");
}

#[test]
#[cfg(unix)]
fn bash_integration_emits_ordered_marks() {
    assert_integration(tf_pty::ShellKind::Bash, &["echo tf-one", "false"], &[0, 1]);
}

#[test]
#[cfg(windows)]
fn pwsh_integration_emits_ordered_marks() {
    assert_integration(
        tf_pty::ShellKind::Pwsh,
        &["Write-Output tf-one", "cmd /c exit 3"],
        &[0, 3],
    );
}

#[test]
#[cfg(windows)]
fn windows_powershell_integration_emits_ordered_marks() {
    assert_integration(
        tf_pty::ShellKind::WindowsPowerShell,
        &["Write-Output tf-one", "cmd /c exit 3"],
        &[0, 3],
    );
}
