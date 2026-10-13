# ADR 0015: Client and account contexts

- Status: Accepted
- Date: 2026-10-12
- Extends: ADR 0008 (storage and configuration)

## Context

A canonical repository path is sufficient to restore terminal layout, but not
to answer "who am I working as?" Consultants and developers commonly use
separate Git authors, GitHub CLI accounts and credentials for personal work,
employers and clients. A visually identical shell with the wrong identity can
produce commits under the wrong email or push through the wrong account.

TermForge must make the selected identity visible without becoming a credential
manager. Private keys, OAuth tokens and passwords already belong to OpenSSH,
Git credential helpers and GitHub CLI.

## Decision

**Portable profiles.** Context profiles live in `config.toml` under
`[[contexts]]`. A profile has a stable lowercase id, display name, badge
colour, expected Git name/email and remote host, plus optional
`GIT_CONFIG_GLOBAL` and `GH_CONFIG_DIR` paths. The default profile is
`personal` and leaves Git identity unconstrained.

**Project assignment.** The selected context id is stored on the canonical
project record in SQLite. This is append-only migration v6. Profile content is
not duplicated in SQLite: assignments survive UI restarts while profiles stay
portable and editable. Moving a project still has ADR 0008's path-identity
limitation.

**Shell boundary.** Newly-created and explicitly restarted local shells
receive `TERMFORGE_CONTEXT`, Git author/committer variables and the two optional
config-directory variables. Existing processes are never mutated. The IPC
allowlists those exact names; arbitrary environment injection is rejected.
Daemon restart metadata stores the validated pairs (append-only migration v7)
so a recovered shell keeps its identity selection.

**Validation and visibility.** TermForge asks the installed `git` executable
for the effective `user.email` and origin URL. The active context is always
shown as a coloured badge in the header and sidebar. An email or remote-host
mismatch is shown in Settings and the status bar. Checks are advisory because
Git configuration can legitimately vary below the project root.

## Consequences

- TermForge selects existing account configuration; it never stores secrets or
  performs GitHub authentication.
- Context changes affect new and restarted shells. A running shell can retain
  its prior environment, which is called out in the picker/settings copy.
- Settings contains an Accounts page for creating, editing, duplicating,
  deleting and assigning profiles. `config.toml` remains the portable source
  of truth, and external edits continue to hot-reload.
- A future stable project UUID can replace path-only identity without changing
  context ids or profile format.
- SSH identity aliases and environment profiles remain separate future work;
  `github_host` validates the remote but does not rewrite it.
