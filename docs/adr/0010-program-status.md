# ADR 0010: Program status uses OSC 7501

- Status: Accepted
- Date: 2026-10-07

## Context

TermForge organizes long-running services and agents around projects. Shell
marks (OSC 133) identify command boundaries and exit codes, while notification
sequences represent one-time events. Neither describes whether a program is
currently working, blocked on the user, done with unseen results, or failed.
Inferring that state from screen contents or window titles is brittle and
requires per-program heuristics.

## Decision

Implement the OSC 7501 Program Status Protocol as session state:

- `tf-tap` validates and parses reports and feature-detection queries.
- `tf-session` owns the bounded hierarchical record set and applies prompt and
  process-exit lifetimes.
- The UI presents the highest-priority current state in terminal chrome.
- `tf status` emits reports for scripts and programs without native support.
- `forged` sends an authoritative status snapshot after scrollback replay so a
  reconnecting UI receives current state even when older raw output was
  truncated.
- A full reset (RIS) clears every record; a soft reset (DECSTR) preserves them,
  as required by the protocol.

The parser has no base64 dependency. This preserves `tf-tap`'s dependency-free
boundary and keeps its untrusted-input surface small.

## Consequences

- Programs and agents can report state without TermForge-specific sockets,
  environment variables, or screen scraping.
- Status works across SSH and containers wherever OSC sequences traverse the
  PTY.
- OSC 133 and OSC 7501 remain complementary: command blocks describe history;
  program records describe current attention state.
- Desktop notifications are a presentation choice triggered by state changes,
  not the stored state itself, and must be rate-limited when added.
- Status snapshots are intentionally semantic IPC events rather than synthetic
  terminal bytes; the daemon remains the authoritative session processor.
