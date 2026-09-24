# ADR 0005: Separate UI and session host (forged)

- Status: Accepted
- Date: 2026-09-24

## Context

Users expect running shells, dev servers and agents to survive a UI crash or update. A single process couples UI bugs to session loss.

## Decision

- `forged` owns PTYs, the OSC tap, the engine state, and the SQLite store.
- `termforge` (UI) connects over local IPC (`tf-proto`) and renders snapshots and deltas.
- `tf` (CLI) talks to `forged` for automation and emits OSC sequences for in-terminal features.
- Phase 0 ships `forged` as a skeleton (store, shell scripts, diagnostics). The IPC server lands in Phase 2; until then the UI may embed `tf-session` directly.

## Consequences

- UI restarts reattach to live sessions; late subscribers get scrollback replay.
- All cross-process data goes through versioned `tf-proto` types.
- There is one extra process to supervise and secure (ADR 0006).
