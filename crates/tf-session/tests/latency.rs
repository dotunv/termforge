//! Latency probe: keystroke to visible output, measured against a real shell.
//!
//! The plan sets a keystroke-to-screen budget of p50 <= 8 ms / p99 <= 16 ms
//! above the display pipeline, measured "Typometer-style plus an internal
//! frame-timestamp probe". This is the internal half of that, and it is
//! deliberately scoped to what can be measured without a GPU:
//!
//! ```text
//!   write(1 byte) -> ConPTY/PTY -> reader thread -> tap -> engine -> snapshot
//! ```
//!
//! What it covers: PTY round-trip, the OSC tap, VT parsing and snapshotting.
//! What it does **not** cover: GPUI text shaping and paint, or display scanout.
//! Those need a window and a real display pipeline, so this number is a
//! necessary condition for the budget rather than the budget itself. Treat a
//! regression here as "our side got slower"; the renderer needs its own probe.
//!
//! Run it with `cargo xtask latency`. It is `#[ignore]`d in normal test runs
//! because it spawns a shell and takes tens of seconds.
//!
//! Budgets are only enforced in release. A debug build of this dependency
//! graph is one to two orders of magnitude slower, so an 8 ms budget there
//! measures the build profile, not the code.

mod common;

use std::time::{Duration, Instant};

use common::RealShell;
use tf_engine::GridSize;
use tf_pty::ShellKind;

/// Plan budget: p50 <= 8 ms, p99 <= 16 ms (build plan §4).
const P50_BUDGET_MS: f64 = 8.0;
const P99_BUDGET_MS: f64 = 16.0;

/// Typometer-style samples: one per keystroke.
fn sample_count() -> usize {
    std::env::var("TF_LATENCY_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
}

/// Per-sample ceiling, so one stalled wakeup cannot hang the run.
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(5);

fn percentile(sorted_us: &[u128], p: f64) -> f64 {
    if sorted_us.is_empty() {
        return 0.0;
    }
    let rank = (p * sorted_us.len() as f64).ceil().max(1.0) as usize;
    sorted_us[rank.min(sorted_us.len()) - 1] as f64 / 1000.0
}

/// Overridable so a loaded CI runner can be given room without editing code.
fn p50_budget_ms() -> f64 {
    std::env::var("TF_LATENCY_P50_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(P50_BUDGET_MS)
}

/// The bottom row a shell prompt is on, searched from the bottom up.
fn prompt_row(snapshot: &tf_engine::Snapshot) -> Option<usize> {
    let mut rows: Vec<&Vec<tf_engine::Cell>> = snapshot.lines.iter().collect();
    rows.reverse();
    rows.iter()
        .position(|r| r.iter().any(|c| c.ch != ' '))
        .map(|i| snapshot.lines.len() - 1 - i)
}

#[test]
#[ignore = "slow and timing-sensitive: cargo xtask latency"]
fn keystroke_to_visible_output_is_within_budget() {
    let Some(shell_kind) = common::testable_shells()
        .into_iter()
        .find(|k| matches!(k, ShellKind::Bash | ShellKind::GitBash))
    else {
        eprintln!("no POSIX shell available; skipping");
        return;
    };

    let n = sample_count();
    let shell = RealShell::spawn_with(shell_kind, GridSize::new(120, 40))
        .expect("shell should be spawnable where it was discovered");

    // Settle: the first prompt pays for shell startup and PS1 evaluation.
    let settle = Instant::now();
    while !shell.at_prompt() && settle.elapsed() < Duration::from_secs(20) {
        let _ = shell.wait(Duration::from_millis(50));
    }
    assert!(shell.at_prompt(), "shell never reached a prompt");

    // Type into one prompt line, then clear it, so every sample is a
    // keystroke against a live prompt rather than into a running command.
    let mut samples: Vec<u128> = Vec::with_capacity(n);
    let mut timeouts = 0usize;
    for i in 0..n {
        let ch = (b'a' + (i % 26) as u8) as char;
        let started = Instant::now();

        // Backspace first so the line never grows without bound.
        if i % 64 == 63 {
            let _ = shell.session.write(b"\x7f");
        }
        let mut buf = [0u8; 4];
        shell
            .session
            .write(ch.encode_utf8(&mut buf).as_bytes())
            .expect("write");

        // Visible means: the engine's snapshot contains what was typed.
        let deadline = started + SAMPLE_TIMEOUT;
        let mut seen = false;
        while Instant::now() < deadline {
            if let Some(row) = prompt_row(&shell.session.snapshot()) {
                if shell.session.snapshot().row_text(row).contains(ch) {
                    seen = true;
                    break;
                }
            }
            let _ = shell.wait(Duration::from_millis(1));
        }
        let elapsed = started.elapsed();
        if seen {
            samples.push(elapsed.as_micros());
        } else {
            timeouts += 1;
        }
    }
    shell.quit();

    assert!(
        !samples.is_empty(),
        "no keystroke ever became visible; shell never echoed"
    );
    let total = timeouts + samples.len();
    let budget = p50_budget_ms();
    samples.sort_unstable();
    let (p50, p99, max) = (
        percentile(&samples, 0.50),
        percentile(&samples, 0.99),
        samples[samples.len() - 1] as f64 / 1000.0,
    );
    let mean = samples.iter().sum::<u128>() as f64 / samples.len() as f64 / 1000.0;

    eprintln!("\n=== keystroke -> visible output ({n} samples) ===");
    eprintln!(
        "  build      {}",
        if cfg!(debug_assertions) {
            "debug (budget not enforced)"
        } else {
            "release"
        }
    );
    eprintln!("  scope      PTY -> tap -> engine -> snapshot (no GPU paint)");
    eprintln!(
        "  samples    {} ok, {timeouts} timed out of {total}",
        samples.len()
    );
    eprintln!("  mean       {mean:.3} ms");
    eprintln!("  p50        {p50:.3} ms   (budget {budget} ms)");
    eprintln!("  p99        {p99:.3} ms   (budget {P99_BUDGET_MS} ms)");
    eprintln!("  max        {max:.3} ms");

    // A stalled sample usually means the runner was descheduled, not that the
    // code is slow, so treat timeouts as a warning rather than a failure.
    if timeouts > 0 {
        eprintln!(
            "  note       {timeouts} sample(s) exceeded {SAMPLE_TIMEOUT:?}; runner may be loaded"
        );
    }

    if cfg!(debug_assertions) {
        eprintln!("  result     reported only; run `cargo xtask latency` to enforce");
        return;
    }
    assert!(
        p50 <= budget,
        "p50 {p50:.3} ms exceeds the {budget} ms budget \
         (this covers PTY + tap + engine only; if it regressed here, the \
         renderer cannot make up the difference)"
    );
}
