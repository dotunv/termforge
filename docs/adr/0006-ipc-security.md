# ADR 0006: IPC is local, per-user and authenticated

- Status: Accepted
- Date: 2026-09-24

## Context

The previous codebases exposed their daemons with no authentication: a TCP port file readable by all users, and a named pipe with the default DACL that also accepted remote clients. Anything that can write to a PTY can run commands as the user.

## Decision

Windows named pipes, created as follows:

- Name: `\\.\pipe\termforge-<user SID>-<channel>`.
- `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first instance, so another process cannot squat the name.
- `PIPE_REJECT_REMOTE_CLIENTS`.
- An explicit security descriptor granting access only to the current user SID (and SYSTEM).
- A random 256-bit token written to `%LOCALAPPDATA%\TermForge\ipc.token` with a user-only ACL. `ClientMsg::Hello` must carry it; mismatches receive `ProtoError::Unauthorized` and are disconnected.
- Frames are capped at 8 MiB and decoded from bytes. Untrusted input is fuzzed (`fuzz/fuzz_targets/proto_frame.rs`).

On Unix, a socket in `$XDG_RUNTIME_DIR` with mode 0600 plus the same token.

## Consequences

- Other local users and remote machines cannot connect.
- The unsafe Win32 calls needed for the security descriptor are confined to one module with `// SAFETY:` comments; that module is the only exception to `unsafe_code = "deny"`.
