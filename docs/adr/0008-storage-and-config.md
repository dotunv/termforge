# ADR 0008: Storage and configuration

- Status: Accepted
- Date: 2026-10-03
- Amends: build plan §5 ADR-8

## Context

TermForge's differentiator is project continuity: reopening a repo restores the
layout, tabs, working directories, profiles, scrollback anchors, services and
notes. That requires durable local state, and it must not require an account.

The Go daemon's storage was one of the things the rewrite deliberately dropped,
but its *ideas* (projects, tasks, events, knowledge, snapshots) are worth
keeping. It also had bugs, so the schema is a rewrite rather than a port.

## Decision

**Storage.** SQLite via `rusqlite` in WAL mode, one database per user, owned by
the session host (`forged`). Schema changes are append-only migrations tracked
with `PRAGMA user_version`, each run inside a transaction. The current v1 schema
has `projects`, `command_history` and `workspace_state`. The append-only v2
migration adds restorable session metadata. This recreates the shell, latest
working directory and dimensions after a daemon restart; it does not claim to
resurrect a process that died with the daemon.

The UI never opens the database. It saves and loads its workspace layout
document (workspace order, names and pins; the daemon treats it as opaque text)
through the `SaveWorkspaceState` and `LoadWorkspaceState` requests, which store
it in `workspace_state` keyed by project. This is protocol v6.

The append-only v3 migration adds project-scoped tasks with a lifecycle state
and bounded context summary. Sessions may reference a task, making the reason a
shell exists durable alongside its technical launch metadata.

**Project identity.** A project is keyed by `dunce::canonicalize` of its root,
stored as plain text. Never a hash, and never re-derived differently on two
sides: `forged`, the UI and the CLI must agree on identity by construction, not
by convention. `dunce` rather than `std::fs::canonicalize` because the latter
returns `\\?\` verbatim paths on Windows, which are unusable as display strings
and unstable across processes.

**Config.** TOML with an accompanying JSON schema so editors give
autocompletion. Reload on change, and surface errors in the UI rather than
crashing or silently ignoring.

**Privacy.** Command contents stay in the local database. They never appear in
telemetry or crash reports. This is a product promise, not an implementation
detail: "nothing you set up is ever lost" and "nothing you type leaves your
machine" have to both be true.

## Consequences

- The session host owns the database, so a UI crash cannot corrupt it. A
  crashed UI reconnects and reads state; it does not open the file.
- WAL mode permits a reader (the UI) alongside the writer (`forged`), which the
  process model needs.
- Migrations are append-only. Editing a released migration would silently
  diverge users' databases, so this is a hard rule in `AGENTS.md`.
- Storing plain paths means a project moved on disk looks deleted. A rename or
  move heuristic is a UI concern, not a schema one.
- The JSON schema is a separate artefact that can drift from the TOML it
  describes. It needs a test asserting the two agree on key names.
- `dunce` paths are display-friendly but not round-trippable through
  `\\?\` APIs. Anything that must hand a path to a Windows API needs to
  re-prepend the prefix deliberately.
- No cloud sync, no teams, no login. Every feature must work offline.
