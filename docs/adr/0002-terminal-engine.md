# ADR 0002: alacritty_terminal behind a TerminalEngine trait

- Status: Accepted
- Date: 2026-09-24

## Context

Writing a correct VT emulator is a multi-year effort. The previous prototype's hand-written parser passed its own tests but had no wide-character support and diverged from xterm behaviour. Mature options: `alacritty_terminal` (Rust, used by Zed), `libghostty-vt` (Zig/C ABI, very fast and correct, Windows support still maturing), `vt100` (simpler, fewer features).

## Decision

`tf-engine` defines a `TerminalEngine` trait (feed, resize, snapshot, cursor line, replies, title) and ships `AlacrittyEngine` built on `alacritty_terminal` 0.26. Replies the emulator wants to send (device attribute responses and similar) are drained by the caller after every feed and written back to the PTY.

## Consequences

- Correctness comes from a battle-tested emulator on day one.
- A `libghostty-vt` engine can be added and A/B tested behind the same trait.
- Snapshots are text-only in Phase 0; attributes are added with the renderer.
