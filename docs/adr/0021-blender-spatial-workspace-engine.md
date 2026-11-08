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
| Operate | Environment \| (Services over Timeline) | where work runs, what is running and listening |

### Operate

Operate is built only from what can be observed, and says so where it cannot
know:

- **Environment**: a clear identity (local, or SSH host), project path, branch,
  context, daemon connection state with its change history, sessions per
  environment, and a capability list (`operate::capabilities`). Remote sessions
  are terminals only today, so Files, Git, service launching and port discovery
  are listed as unavailable with the reason rather than showing local data
  under a remote name.
- **Services**: every session with its observed state, and for processes
  TermForge started (`Managed`) the actions it can really perform: open shell,
  follow output, stop (Ctrl-C into that shell) and restart. State comes from
  the shell's command marks; processes TermForge did not start are not tracked,
  and the area says so. Launchers are the project's tasks with commands and
  agent CLIs found on `PATH`.
- **Listening ports** (TCP): read from `/proc/net/tcp*` on Linux off the UI
  thread every few seconds while a Services area is visible. Each port shows its
  owning process when readable, how widely it is reachable (this machine only,
  all interfaces), and a copy-URL action. A port is associated with the project
  only when its owner's working directory is inside the project; processes owned
  by other users stay "owner unknown". Other platforms report that port
  discovery is not implemented instead of showing an empty list.
- **Containers**: read from `docker` (or `podman`) with a 5-second limit, so a
  hung daemon cannot freeze anything. Rows show state, image, status and
  published ports. A container belongs to the project only when Compose started
  it from a directory inside it (the `working_dir` label); the project's compose
  and Dockerfile files are listed. Actions are the user's explicit choice:
  start, stop and restart run the runtime CLI off the UI thread, and logs and
  shell open a managed session (`logs -f`, `exec -it`) so output, state and
  stop work like any other service. Ids are only ever passed to the CLI or a
  shell when they are alphanumeric. Absent runtime, unreachable daemon and
  remote sessions each say so.
- **Timeline**: an activity log of observed changes only: service state
  changes, project ports opening or closing, container state changes, daemon
  connection changes and new language-server diagnostics. The first observation
  of anything is a baseline, not an event. It can be filtered by text, by
  source and by level, and events that point at something (a service's tab, a
  diagnostic's file) open it. The log keeps the latest 500 events in memory and
  is not saved across launches.

Not built: tunnels and port forwarding, compose-level actions (up, down),
persisted or searchable log history, search inside a container's own log
stream beyond the terminal's search, remote file/Git/process access, cloud
providers, and stopping a process the user did not start from TermForge.

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
- Build's layout is the tab's pane tree and is saved with the tab (`S` lines),
  as before.
- The graph canvas view (lens, zoom, pan) is saved in a `V` line, clamped on
  load; unknown lenses are ignored. Scroll positions and the selected node are
  not saved.
- **View: reset workspace layout** (command palette) returns a session-free
  workspace to its default layout. In Build it only evens out the splits: every
  pane and session is kept, because resetting a layout must never close a
  process.

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
- Not yet built: work items, and environment identity beyond what Operate
  shows.

## Amendment: content-led workbench styling

- Date: 2026-11-07
- Status: Accepted

Work now defaults to a single Task Board area, with task details and suggestions
inside that surface. The former stock 68/32 Task Board–Inspector split is upgraded
when restored; other saved compositions keep their editor choices and geometry.
The inspector remains available from the area selector and split controls.

The default dark palette uses neutral graphite and a restrained blue accent,
while custom theme overrides remain intact. Workspace navigation uses subtle
selection fills and an accent underline. Interface text defaults to the native
system font, with 13px body text and 12px secondary labels; code and terminal
content keep the configured monospace font. Context colours are small identity
markers rather than filled badges. Shared toolbar and area controls use a
font-independent vector stroke grid. Content areas have a small outer inset and
subtle boundaries, following the hierarchy recommendations from the WebStorm
assessment. Area focus is shown in the area header rather than with an accent
frame around the whole document. Explore uses
a compact file toolbar and narrower explorer, and Review uses a narrower change
rail so source and diffs have more room. Navigation surfaces recede from the
canvas in both light and dark themes; user base and accent choices still apply.
This changes presentation only: session ownership and layout encoding are unchanged.

## Amendment: purpose-specific editor areas

- Date: 2026-11-07
- Status: Accepted

The area chooser groups views by Develop, Explore and Operate, filters by name
and purpose, and owns keyboard input while open. Only mountable editor types
are offered: Terminal in Build, session-free views elsewhere. Search, selection
and scrolling are transient view state; editor codes and saved layouts stay valid.

Graph camera and lens changes now update the session-free tree that owns the
visible graph, rather than the Build session tree. Inspector follows visible
graph selection, then selected task/file context, with project details as a
fallback. Code Editor hides its built-in file rail when a separate Outliner is
mounted. Supporting areas use readable introductions, wrapping details and
explicit availability states. The unused Timeline pause control is removed;
its view remains an observed event log rather than implying it pauses collection.

Opening a file from an explorer, diagnostic or graph reveals an existing code
area, or splits beside the source view so navigation remains available. At the
area limit it reuses the focused session-free area; it never replaces a terminal.
Smart-group file rows open files and directory rows reveal the normal tree.
Graph cards use reduced metadata at lower zoom levels to retain readable labels
without overlapping their neighbours.

## Amendment: navigable project explorer

- Date: 2026-11-07
- Status: Accepted

The conventional folder tree is the default for new workspaces. Its first scan
collapses directories, revealing ancestors of an already open file; refresh
preserves expansion choices. A shared browser model supplies both the embedded
file rail and standalone Outliner, with path search, keyboard navigation,
active-file reveal, relative-path copying and independent list scrolling.
Ctrl+P (or Command+P) focuses file search when a file view is present; terminal
workspaces retain their shell key handling. Search reaches collapsed folders
and lists files with their parent paths, with at most 200 rendered results.
The cancellable scanner indexes up to 20,000 entries and 16 directory levels,
continues respecting ignores and does not follow directory symlinks. Reaching
the entry limit is shown in the browser. Query and navigation focus are
transient; no file mutations, dependencies or storage migrations are introduced.

## Amendment: language-aware file marks

- Date: 2026-11-08
- Status: Accepted

Explorer rows, file search, smart groups, change lists and the open-file header
use shared filename-based language/file-type marks. Common source extensions,
compound TypeScript extensions, dotfiles and extensionless project files are
recognised; unknown files retain the document icon. Identification does not
imply syntax-highlighting or language-server availability. File icon colours
have separate semantic theme slots with contrast checks and optional theme
file overrides, so file types do not borrow warning/error colours. The marks
are rendered locally and add no icon-font, asset download or crate dependency.
