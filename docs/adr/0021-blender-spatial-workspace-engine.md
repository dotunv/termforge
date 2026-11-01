# ADR 0021: Blender-style polymorphic area tree and spatial workspace presets

- Status: Accepted
- Date: 2026-10-29
- Extends: ADR 0012 (split panes), ADR 0017 (workspace environments)

## Context

`PaneTree` originally modelled only split terminals (`Leaf(PaneId)`), while the
project workbench used fixed page layouts for Overview, Files, Git, Tasks and
Sessions. Users also hit a protocol version mismatch when connecting a newer UI
(`PROTOCOL_VERSION = 10`) to a running `forged` daemon (`v9`), which surfaced
as an opaque `"expected welcome, got Response { ... VersionMismatch ... }"`
error instead of a structured daemon status in the status bar.

## Decision

- `IpcClient::connect` in `tf-ipc` maps `ServerMsg::Response { result: Err(error), .. }`
  received during the `Hello` handshake directly to `IpcError::Protocol(error)`,
  preserving `ProtoError::VersionMismatch { server, client }`.
- `remote_session` tracks the latest daemon connection status (`Connected`,
  `Offline`, `VersionMismatch { server, client }`) and surfaces a non-blocking
  indicator in the bottom status bar while keeping the workspace responsive via
  the embedded session fallback.When a version mismatch is detected, `termforge`
  replaces the stale `forged` instance with the matching bundled executable.
- `PaneTree` in `bins/termforge/src/panes.rs` is generalised into a polymorphic
  area binary tree. Every leaf holds an `AreaNode` with a unique `AreaId`
  (`PaneId`) and an `EditorKind`:
  - `EditorKind::Terminal(PtySessionId)`
  - `EditorKind::GraphCanvas(CanvasState)`
  - `EditorKind::Inspector(InspectorState)`
  - `EditorKind::Outliner(ProjectTreeState)`
  - `EditorKind::Timeline(LogStreamState)`
- Each area renders a standardized header strip with an in-place editor-type
  selector dropdown, editor-specific context actions, and split/maximize/close
  controls.
- `bins/termforge/src/workspace.rs` defines `WorkspacePreset` split-tree
  configurations (`TopologyFlow`, `ConsoleMatrix`, `StateDebug`). Switching
  presets reconfigures the area split tree without terminating or detaching live
  PTY sessions.
- The central spatial viewport (`GraphCanvas`) provides a 2D pan/zoom coordinate
  transform (`CanvasState`) and renders interactive system nodes (`forged`
  daemon, Git branches/worktrees, active shell sessions, and project tasks) that
  route inspection and terminal focus on selection.

## Consequences

- Every area in a split tree can switch between Terminal, GraphCanvas,
  Inspector, Outliner and Timeline in place without destroying terminal sessions.
- Existing `PaneTree` compact encoding (`1`, `h0.500(1,2)`) remains backward
  compatible; editor kinds are encoded alongside area leaves when non-default.
- Daemon offline or version-mismatch states never block or freeze UI rendering.

