# ADR 0018: Files is an editor backed by language servers

- Status: Accepted
- Date: 2026-10-22
- Extends: ADR 0001 (UI framework), ADR 0016 (project workbench)

## Context

A Files workspace is only useful when it can carry work from navigation through
editing and validation. A decorative source preview would make the workspace
look complete while leaving its central interaction missing. Editing also
cannot be added responsibly as a collection of file-text fields: buffers need
dirty state, atomic saves, undo/redo, external-change handling, diagnostics and
language-aware positions.

## Decision

- Introduce `tf-editor`, a framework-free editor model that owns UTF-8 buffers, selections,
  edit transactions, undo/redo, dirty state and atomic saves. GPUI remains a
  renderer and input adapter in `bins/termforge`.
- Add a language-service host outside the UI process. It launches one server
  per language/project configuration, speaks LSP over stdio, versions document
  updates and restarts failed servers without risking terminal sessions.
- Use UTF-16 LSP positions only at the protocol boundary. The editor model keeps
  byte and grapheme-aware coordinates and performs explicit conversions.
- The Files environment owns an explorer, editor groups, breadcrumbs and a
  contextual outline/diagnostics panel. Problems are shared with the Tasks and
  Terminal environments through a read model rather than copied UI state.
- Opening a file is safe before the editor model lands: bounded UTF-8 files may
  be viewed, but the UI must not imply that an inert preview is editable.

## Consequences

- The first Files layout can establish navigation and viewing without shipping
  a misleading editor. Editing is enabled only when buffer, persistence and
  undo invariants are tested.
- `tf-editor` has no UI or async-runtime dependency. LSP support adds process
  supervision and protocol types; the exact host process requires a follow-up
  amendment before that part is implemented.
- Saves use a same-directory temporary file, flush file content before rename,
  and sync the containing directory where supported. Windows uses a recoverable
  backup-and-replace sequence because stable `std` does not expose `ReplaceFileW`.
- Syntax highlighting should come from a parser/highlighter abstraction and
  must remain useful when no language server is installed.
