//! Spike A from the build plan: prove that OSC 133 marks stay ordered
//! relative to output under ConPTY over a long run of real commands.
//!
//! Warp found that ConPTY swallowed unknown DCS sequences, emitted OSC out
//! of order relative to output, and kept its own cached grid state — bad
//! enough that they had to fork it. The plan asks for 10,000 commands per
//! shell to catch that class of bug before users do.
//!
//! The long run is `#[ignore]`d because it is far too slow for `cargo xtask ci`.
//! Run it explicitly:
//!
//! ```sh
//! cargo test -p tf-session --test spike_a -- --ignored --nocapture
//! ```
//!
//! Overrides: `TF_SPIKE_COMMANDS` (default 10,000), `TF_SPIKE_SHELLS`
//! (comma-separated debug names, default every supported shell).

mod common;

use std::time::Duration;

use common as harness;
use common::{RealShell, Skip};
use tf_pty::ShellKind;
use tf_session::Block;

/// Commands with a known exit status, per shell family.
#[derive(Clone, Copy)]
struct Dialect {
    /// Prints a line and exits 0.
    ok: &'static str,
    /// Exits 1.
    one: &'static str,
    /// Exits 3.
    three: &'static str,
}

const POSIX: Dialect = Dialect {
    ok: "echo tf-ok",
    one: "false",
    three: "sh -c 'exit 3'",
};

const PWSH: Dialect = Dialect {
    ok: "Write-Output tf-ok",
    // `cmd /c exit N`, not `exit N`: at an interactive PowerShell prompt
    // `exit` terminates the shell, so the block never gets its D mark and the
    // harness would spin until it timed out.
    one: "cmd /c exit 1",
    three: "cmd /c exit 3",
};

fn dialect(kind: ShellKind) -> Dialect {
    match kind {
        ShellKind::Pwsh | ShellKind::WindowsPowerShell => PWSH,
        _ => POSIX,
    }
}

/// Exit codes cycle 0, 1, 3 so each block asserts a *distinct* expected
/// status. A harness that only ever ran `echo` would still pass if
/// exit-code plumbing were broken.
fn expected_code(n: usize) -> i32 {
    match n % 3 {
        0 => 0,
        1 => 1,
        _ => 3,
    }
}

fn command_for(d: Dialect, n: usize) -> &'static str {
    match n % 3 {
        0 => d.ok,
        1 => d.one,
        _ => d.three,
    }
}

/// The ordering invariant: prompt, then command, then end — with the right
/// exit status. This is what breaks if ConPTY reorders OSC against output.
fn check(b: &Block, n: usize) -> Result<(), String> {
    let want = expected_code(n);
    if b.exit_code != Some(want) {
        return Err(format!(
            "command {n}: exit code {:?}, expected {want}; block {b:?}",
            b.exit_code
        ));
    }
    let (a, c, d) = (b.prompt_line, b.output_line, b.end_line);
    let (Some(c), Some(d)) = (c, d) else {
        return Err(format!(
            "command {n}: missing anchors output={c:?} end={d:?}; block {b:?}"
        ));
    };
    if !(a <= c && c <= d) {
        return Err(format!(
            "command {n}: marks out of order A={a} C={c} D={d}; block {b:?}"
        ));
    }
    Ok(())
}

fn wanted_count() -> usize {
    std::env::var("TF_SPIKE_COMMANDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000)
}

fn wanted_shells() -> Vec<ShellKind> {
    let all = harness::testable_shells();
    let Ok(filter) = std::env::var("TF_SPIKE_SHELLS") else {
        return all;
    };
    let named: Vec<String> = filter.split(',').map(|s| s.trim().to_lowercase()).collect();
    all.into_iter()
        .filter(|k| named.contains(&format!("{k:?}").to_lowercase()))
        .collect()
}

/// PowerShell prompts are far slower than POSIX shells, so scale with the
/// workload. Kept generous: the budget only matters when something is wrong,
/// and too tight a bound turns a loaded runner into a false failure.
fn budget(count: usize) -> Duration {
    Duration::from_secs(120 + (count as u64) / 10)
}

#[test]
#[ignore = "slow: cargo test -p tf-session --test spike_a -- --ignored"]
fn osc_marks_stay_ordered_over_ten_thousand_commands() {
    let count = wanted_count();
    let shells = wanted_shells();
    assert!(!shells.is_empty(), "no testable shells selected");

    let mut failures = Vec::new();
    let mut ran = 0;
    eprintln!("\n=== Spike A: {count} commands per shell ===");
    for kind in shells {
        let shell = match RealShell::spawn(kind) {
            Ok(s) => s,
            Err(Skip::NotInstalled) => {
                eprintln!("  {:<20} skip (not installed)", format!("{kind:?}"));
                continue;
            }
            Err(e) => {
                let msg = format!("{kind:?} is {e} but is a supported shell");
                eprintln!("  {:<20} FAIL  {msg}", format!("{kind:?}"));
                failures.push(msg);
                continue;
            }
        };
        let d = dialect(kind);
        ran += 1;
        match harness::drive(
            &shell,
            count,
            budget(count),
            |n| command_for(d, n).to_owned(),
            check,
        ) {
            Ok(elapsed) => eprintln!(
                "  {:<20} pass  {:.1}s",
                format!("{kind:?}"),
                elapsed.as_secs_f64()
            ),
            Err(e) => {
                eprintln!("  {:<20} FAIL  {e}", format!("{kind:?}"));
                failures.push(e);
            }
        }
        shell.quit();
    }

    assert!(ran > 0, "no supported shell was available to test");
    assert!(
        failures.is_empty(),
        "Spike A failed for {} shell(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// The cheap guard: ordering must hold in the common case, and this runs in
/// normal CI on every PR.
#[test]
fn marks_stay_ordered_for_a_handful_of_commands() {
    let count = 12;
    for kind in harness::testable_shells() {
        let shell = match RealShell::spawn(kind) {
            Ok(s) => s,
            Err(Skip::NotInstalled) => {
                eprintln!("{kind:?} not installed; skipping");
                continue;
            }
            Err(e) => panic!("{kind:?} is {e} but is a supported shell"),
        };
        let d = dialect(kind);
        harness::drive(
            &shell,
            count,
            Duration::from_secs(120),
            |n| command_for(d, n).to_owned(),
            check,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        shell.quit();
    }
}
