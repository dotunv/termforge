#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns
//! End-to-end tests of the processing pipeline.

mod common;

use common::RealShell;
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

/// Real shell, real PTY, real integration script. Each command must produce
/// A <= C <= D marks in order with the right exit code.
///
/// The long 10,000-command version of this lives in `spike_a.rs`; this is the
/// cheap version that runs on every PR.
fn assert_integration(kind: tf_pty::ShellKind, commands: &[&str], codes: &[i32]) {
    use std::time::Duration;

    let shell = match RealShell::spawn(kind) {
        Ok(s) => s,
        Err(common::Skip::NotInstalled) => {
            eprintln!("{kind:?} not installed; skipping");
            return;
        }
        Err(e) => panic!("{kind:?} is {e} but is a supported shell"),
    };

    common::drive(
        &shell,
        commands.len(),
        Duration::from_secs(90),
        |n| commands[n].to_owned(),
        |_, _| Ok(()),
    )
    .unwrap_or_else(|e| panic!("{e}"));

    // Retained blocks are ordered oldest-first, so they line up with `codes`.
    let cmds = shell.finished_commands();
    assert_eq!(
        cmds.len(),
        commands.len(),
        "{}",
        common::diagnostics(&shell, commands.len(), commands.len())
    );
    for (b, code) in cmds.iter().zip(codes) {
        assert_eq!(b.exit_code, Some(*code), "block {b:?}");
        let (a, c, d) = (b.prompt_line, b.output_line.unwrap(), b.end_line.unwrap());
        assert!(a <= c && c <= d, "marks out of order: {b:?}");
    }
    assert!(shell
        .session
        .take_events()
        .iter()
        .any(|e| matches!(e, SessionEvent::Cwd(_))));
    shell.quit();
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
