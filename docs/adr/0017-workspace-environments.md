# ADR 0017: Workspace environments are stable project layouts

- Status: Accepted
- Date: 2026-10-22
- Extends: ADR 0016 (project workbench)

## Context

The project workbench introduced artifact tabs, but project activities still
competed for space in generic panels. In particular, reviewing a change opened
its diff in the narrow contextual inspector. Developers need to move between
terminal work, files, tasks and review without recreating sessions or manually
rebuilding a useful panel arrangement each time.

Blender's workspaces provide a useful interaction model: each top-level tab is
an environment with a layout suited to one activity, while all tabs continue
to operate on the same underlying project.

## Decision

- Workbench artifact tabs are workspace environments. Switching one changes
  the visible composition, not the project, PTYs, tasks or daemon attachment.
- Changes is a first-class environment with a changed-file rail and a large
  unified-diff surface. It reads the repository already associated with the
  active session and reuses the existing Git status and diff functions.
- Each environment owns its complete center composition. Project uses a
  dashboard with a compact terminal dock; Git uses a change rail and review
  surface; Files owns an explorer/editor/context layout; Tasks owns planning
  and suggestions; Terminal gives the pane tree the full work area.
- Hiding the terminal in another environment never detaches it. Its sessions,
  processes and pane tree remain owned by `forged` and the existing UI state.
- Environment-specific selection is view state owned by `TerminalView`. It is
  not persisted or added to the IPC protocol in this slice.
- Diff colors use existing semantic theme slots; the environment introduces no
  raw palette and no new dependency.

## Consequences

- Review gets enough horizontal space for real diffs without turning the
  contextual inspector into a second editor.
- Refreshing the Changes environment executes local Git commands synchronously,
  as the pre-existing Changes dock already did. A future repository model can
  move this work off the UI thread without changing the environment contract.
- Environment layouts are fixed for now. User-created environments, reordered
  tabs and persisted panel geometry are follow-up work.
