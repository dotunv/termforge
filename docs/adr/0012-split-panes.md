# ADR 0012: Split panes are a per-workspace pane tree

- Status: Accepted (implemented; not yet exercised in the running app)
- Date: 2026-10-10

## Context

A workspace (sidebar entry) currently owns exactly one session, and
`TerminalView` holds the selection, search, scroll, hover and grid-bounds state
for the one session on screen. Developers expect to see a shell, a server and a
test watcher side by side without creating three workspaces.

## Decision

- A workspace owns a `PaneTree` (`bins/termforge/src/panes.rs`): a binary tree
  whose leaves are pane ids and whose interior nodes are horizontal or vertical
  splits with a ratio. The tree is plain data with no GPUI or session types, so
  geometry, focus movement and persistence are unit-tested without a display.
- Each pane has its own session. Panes are not subdivisions of one PTY, so
  every pane gets its own scrollback, command blocks, program status and exit
  state, and a pane's shell can be restarted independently.
- Per-pane view state (selection, search, scroll, hover, grid size, hitbox)
  moves out of `TerminalView` into a `Pane` struct. `TerminalView` keeps
  window-level state (sidebar, palette, settings, theme).
- Focus moves by geometry (`neighbor`), not tree order, so the key that moves
  focus right goes to the pane that is visibly to the right.
- Depth is capped (16) and ratios are clamped to 10%..90%, so a corrupt layout
  file cannot produce unusable or unbounded trees.
- Persistence: the layout file gains the encoded tree per workspace. `forged`
  already recreates sessions by id; the UI reattaches each pane's session and
  falls back to a single pane if the tree does not decode or names a session
  the daemon does not have.
- Pane close follows the sibling-takes-space rule; closing the last pane closes
  the workspace.

## Implementation

- `TerminalView` keeps showing one *focused* pane through its existing flat
  fields. Other panes are parked in `WorkspaceTab::panes` as `PaneSlot`s, and
  moving focus swaps the two, the same mechanism workspace switching already
  used. This avoided duplicating selection, search and scroll state per pane,
  at the cost of the per-pane state living in two places.
- Parked panes are painted by a read-only canvas (no selection, links or
  search overlay) and focus on click. Their sessions are drained like
  background workspaces, so cwd, exit status and attention keep updating.
- The layout file gets an optional `S` line per workspace (tree, focus index,
  session ids). Older builds ignore it; a tree that does not decode, or names a
  session the daemon no longer has, restores as a single pane and the
  surviving sessions reappear as their own workspaces.
- Keys: Ctrl+Shift+D / E split right / down, Ctrl+Shift+X close pane,
  Ctrl+Alt+Arrows move focus, Ctrl+Alt+Shift+Arrows resize. A cap of 8 panes
  applies.

## Pointer and attention behaviour

- Dividers are 6px grab handles centred on each split; dragging converts the
  pointer position into the split's ratio (clamped to 10%..90%) using the
  bounds recorded from the last paint. The layout is saved when the drag ends.
- The wheel scrolls the pane under the pointer. For a parked pane this is
  scrollback only: full-screen programs there are not sent keys or mouse
  reports until the pane is focused.
- A parked pane's border takes the colour of its highest pending attention
  level until it is focused, including in the active workspace.
- A split restores only if every pane's session is running and reachable from
  this machine, decided before anything is attached, so a failed restore
  degrades to separate workspaces rather than stranding sessions.

## Known gaps

- Search, rename and the palette act on the focused pane/workspace only.
- A pane that fails to reattach for a reason other than a missing SSH client
  would still be left running in the daemon without a workspace.

## Consequences

- The sidebar's per-workspace status must aggregate over panes (worst
  attention state wins).
- Resize must propagate to every pane's session, not only the active one.
- The layout file keeps version 1; pane data is an additive line type.
