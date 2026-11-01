# ADR 0021: Workspaces as layouts of areas, with sessions kept apart

- Status: Accepted (supersedes the first draft of this ADR)
- Date: 2026-11-01
- Extends: ADR 0012 (split panes), ADR 0017 (workspace environments)

## Context

The workbench had two competing models: fixed top-bar pages (Project, Explore,
Review, Work, Terminal) and a Blender-style tree of areas that could host any
editor. The area tree gave every area its own shell session, so a Task Board
or Inspector area owned a process, switching layouts spawned hidden shells, and
closing any area closed a session. The layout presets that tested the idea were
never reachable from the UI.

Separately, connecting a newer UI (`PROTOCOL_VERSION = 10`) to an older
`forged` surfaced an opaque handshake error.

## Decision

### Five workspaces

The top bar lists the activities of the product: **Work, Explore, Build,
Review, Operate** (`workspace::Workspace`). A workspace is a layout of areas,
not a page. The same project, sessions and selected work travel between them.

| Workspace | Default layout | Shows |
| --- | --- | --- |
| Work | Task Board \| Inspector | tasks and what needs attention |
| Explore | Code Editor | project files and source |
| Build | the active tab's terminal tree | terminal sessions |
| Review | Git Diff | working-tree changes |
| Operate | Agent Fleet \| Timeline | daemon, agents and live activity |

Operate currently shows only what exists today (daemon status, sessions,
agents, activity). Services, ports and remote environments are not built yet.

### Areas never own processes unless they are terminals

- `EditorKind::Terminal` is the only area kind that owns a session.
- Only **Build** holds terminals. Its layout is the active tab's `PaneTree`,
  which stays terminal-only; sessions are parked and focused there exactly as
  before.
- Every other workspace has a **session-free layout**
  (`TerminalView::workspace_layouts`). Splitting, retyping, resizing, closing
  or switching away from these areas never starts, moves or closes a session,
  and the area-type selector does not offer Terminal outside Build.
- The last area of a workspace cannot be closed.
- Switching workspaces (`switch_workspace`) changes the visible layout and the
  data its areas show. Tests assert the session count and the Build tree are
  unchanged by switching.

### Persistence

- The active workspace is saved in the existing `W` line. Ids written by older
  builds (`tasks`, `files`, `git`, `terminal`, `project`) still load.
- Session-free layouts are saved as `L<TAB>workspace<TAB>tree` lines using the
  existing compact tree encoding (`h0.680(1:k,2:i)`). Older builds ignore these
  lines. Malformed trees, and trees that would mount a terminal, are ignored in
  favour of the default; saved ids are renumbered so they cannot collide with
  pane ids in this window.
- Editor state (canvas pan and zoom, scroll positions) is not saved yet.

### Protocol handshake

- `IpcClient::connect` maps a `Response { result: Err(..) }` received during the
  `Hello` handshake to `IpcError::Protocol(error)`, preserving
  `ProtoError::VersionMismatch { server, client }`.
- `remote_session` tracks the daemon status (`Connected`, `Offline`,
  `VersionMismatch`) and shows a non-blocking indicator in the status bar while
  the workspace keeps working on the embedded session fallback.

## Consequences

- One navigation model. The Blender-style area tree is the layout engine under
  the workspaces, not a second set of pages.
- Hidden shells and session-killing area closes are gone for non-terminal areas.
- Terminal areas cannot yet be placed in workspaces other than Build. Mounting
  an existing session by reference (a read-only or shared view) is future work
  and would need an ownership rule for who may close it.
- The graph canvas, inspector, outliner and timeline remain available as area
  types in session-free layouts but are no longer part of any default layout
  except the inspector in Work and the timeline in Operate.
- Not yet built: work items, environment identity beyond the project name,
  per-workspace saved layouts for Build, and a user-facing way to reset a
  workspace to its default layout.
