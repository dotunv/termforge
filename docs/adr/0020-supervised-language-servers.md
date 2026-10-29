# ADR 0020: Language servers are supervised by the desktop client

- Status: Accepted
- Date: 2026-10-28
- Amends: ADR 0018 (editor and language intelligence)

## Context

ADR 0018 left the exact language-service host unresolved and proposed a host
outside the UI process. The Files editor now needs live diagnostics, but
placing an LSP server in `forged` would couple project-local developer tools to
the durable terminal daemon and require relaying high-volume document text
through the session protocol. A language server is already a separate OS
process; the missing isolation is between its blocking stdio and GPUI.

## Decision

- TermForge owns a small supervisor on background threads and launches each
  language server as a separate child process in the project root. No LSP I/O,
  process wait, or initialization work runs on the GPUI thread.
- The selected Files buffer is synchronized with versioned `didOpen`, full-text
  `didChange`, and `didSave` notifications. Diagnostic versions and supervisor
  generations reject stale results after edits, file switches, or restarts.
- Server discovery is optional and PATH-based. Rust, TypeScript/JavaScript,
  Python, Go, and C/C++ use their conventional language servers when installed;
  editing and syntax highlighting continue without one.
- JSON-RPC frames and editor files are bounded. Initialization has a timeout,
  shutdown is graceful before forced termination, and unexpected exits restart
  with bounded exponential backoff.
- `forged` remains responsible only for durable terminal sessions. Language
  servers share the desktop client's lifetime and cannot interrupt those
  sessions if they fail.

## Consequences

- Diagnostics can appear in the editor and Problems tool without adding an
  async runtime or protocol dependency to `tf-editor`.
- Closing TermForge stops its language servers; reopening a file discovers and
  initializes a fresh server rather than attempting process reattachment.
- Full-text synchronization is intentionally simple and correct for the current
  2 MiB buffer limit. Incremental synchronization can be added after edit deltas
  become first-class in `tf-editor`.
