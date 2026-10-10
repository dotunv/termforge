# ADR 0011: Remote workspaces build on system SSH and location-aware state

- Status: Accepted
- Date: 2026-10-08

## Context

TermForge needs to organise development that spans local machines, SSH hosts,
tailnets, editors, services and coding agents. Tailscale already provides
device identity and encrypted reachability, while OpenSSH already owns host-key
verification, authentication agents, certificates, proxy jumps and tunnels.
Reimplementing either would expand TermForge's secret-handling surface without
improving its project-continuity differentiator.

The existing session model assumes every working directory belongs to the
local filesystem. That is incorrect once terminal output comes from SSH: OSC 7
contains a hostname, but `tf-tap` currently discards it and `forged` persists
the remaining path as a local restart directory.

## Decision

- Use the installed OpenSSH client and its existing configuration and agent.
- Represent OSC working directories as `{ host, path }`, retaining the host.
- Mark daemon sessions with an optional SSH host alias.
- Never canonicalise, validate or persist a remote cwd as a local path.
- Persist the SSH alias so a daemon restart may recreate the connection, but
  do not claim to resurrect the remote process.
- Keep Tailscale transport-neutral: a MagicDNS name is simply an SSH alias or
  hostname from TermForge's perspective.
- Do not store passwords or private keys. A future credential vault requires a
  separate threat model and ADR.

The first UI-independent selection mechanism is `TERMFORGE_SSH_HOST=<alias>`.
It exists to exercise the complete architecture before a host picker lands.

## Consequences

- Host-key prompts and authentication behave exactly as they do in a user's
  normal OpenSSH client.
- `~/.ssh/config`, `ProxyJump`, certificates, ssh-agent and Tailscale addresses
  continue to work without TermForge-specific parsers in the connection path.
- SSH sessions survive UI restarts because `forged` owns the local SSH PTY.
- They do not survive a `forged` crash; that requires an optional remote peer.
- Remote shell integration is not automatically installed. Until it is, an
  SSH connection is one local command block and remote cwd is available only
  when the remote shell emits OSC 7 itself.
