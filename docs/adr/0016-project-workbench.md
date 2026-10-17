# ADR 0016: The project workbench contains the terminal

- Status: Accepted
- Date: 2026-10-15
- Extends: ADR 0001 (UI framework), ADR 0008 (workspace state), ADR 0012 (split panes)

## Context

TermForge began with a terminal-first window: the sidebar exposed projects,
tasks and SSH hosts, but the terminal occupied the entire working area. That
made durable project concepts feel like terminal accessories even though the
product model already treats every repository as a workspace.

The application needs a stable place for project state, tasks, sessions,
account context and attention without weakening terminal compatibility or
moving session ownership out of `forged`.

## Decision

- The main window is a project workbench. It composes project navigation,
  central artifact tabs, a contextual inspector, a bottom tool dock and the
  existing status bar.
- Workspace artifact tabs and terminal session tabs are separate concepts.
  Overview, Tasks and Sessions are the initial artifacts; selecting a session
  changes the terminal model without turning that shell into an artifact tab.
- The terminal is one tool in a dock alongside Problems and Output. The dock
  may be collapsed or maximised temporarily, but presentation changes never
  recreate or detach a session.
- Existing `PaneTree` splits live inside the terminal panel. Panes remain
  independent PTYs with their existing focus, scroll and attention behavior.
- The inspector follows the selected artifact. Its fields are read models of
  existing session, task, repository, context and OSC 7501 state; they do not
  duplicate durable data or open the SQLite store from the UI.
- GPUI workbench components stay in `bins/termforge`; framework-free crates and
  the authenticated daemon boundary are unchanged.
- All colors continue to come from semantic `tf-ui` theme slots, and the
  configured interface and terminal fonts continue to apply.

## Consequences

- Opening a repository now starts in a project overview with the terminal
  available below it instead of making the terminal the whole workspace.
- Switching a session from the Sessions artifact uses the existing lifecycle, so
  reattachment, SSH, account contexts, tasks, program status and persistence
  keep their current semantics.
- The UI still has one large `TerminalView` state owner. The new workbench
  module establishes a composition boundary, but extracting independent
  document and panel stores remains later work.
- The initial expanded dock height and inspector width are fixed. Drag resizing,
  persisted sizing, inspector collapse and narrow-window adaptation require
  follow-up work.
- The existing opaque workspace layout format remains valid; this slice adds
  no protocol change and no store migration.
