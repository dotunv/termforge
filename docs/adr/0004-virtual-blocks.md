# ADR 0004: Blocks are virtual, derived from OSC 133 marks

- Status: Accepted
- Date: 2026-09-24

## Context

Warp-style command blocks make output navigable, copyable and shareable. Warp implements them with a separate grid per block, which requires owning the prompt and diverges from how full-screen apps and shells expect a terminal to behave.

## Decision

Keep one real terminal grid per session. The shell integration scripts emit OSC 133 `A` (prompt), `B` (input), `C` (output) and `D;code` (finished). `tf-tap` observes these without altering the stream and reports the byte offset of each sequence. `tf-session` feeds the engine up to that offset and records the absolute cursor line, so each mark is anchored exactly. `BlockIndex` builds blocks from marks and tolerates missing or extra marks.

## Consequences

- Blocks work with any prompt (Starship, Oh My Posh) and never break full-screen apps.
- The UI draws block chrome (headers, exit status, hover actions) as an overlay over line ranges.
- Blocks disappear gracefully when a shell has no integration.
- Line anchors are absolute grid lines (scrollback + screen). Anchors are kept valid by two rules: when scrollback shrinks (`ESC [3J`), anchors shift up and blocks that fell off are dropped; when a prompt appears at or above an existing block (`clear`, Ctrl+L), those blocks are dropped because the screen was rewritten.
- Known gap: lines trimmed at the scrollback cap (10,000 by default) are not detected yet, so blocks can drift after very long sessions. Fix planned with a monotonic line counter in `tf-engine`.
