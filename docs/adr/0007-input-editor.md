# ADR 0007: v1 relies on the shell's own line editor

- Status: Accepted
- Date: 2026-10-03
- Amends: build plan §5 ADR-7

## Context

Warp owns the prompt line with a rich editor: multi-cursor editing, syntax
highlighting, completions from Fig specs, and commands submitted as whole
structured objects. That is a large part of why Warp feels different from a
terminal.

It is also a large amount of compatibility risk. The shell's own line editor
(PSReadLine, readline, zle, fish) already handles quoting, history search,
multiline continuation, bracketed paste, and every per-shell quirk users depend
on. Replacing it means reimplementing all of that, and Warp had to write a
PowerShell integration from scratch to do it.

TermForge's differentiator, per the build plan, is project continuity — "open a
repo and get back your exact layout" — not out-editing Warp at the prompt.

## Decision

v1 does not implement its own input editor. TermForge renders the prompt line
and forwards keystrokes to the shell, adding only OSC 133 click-to-move-cursor,
the approach Ghostty 1.3 uses.

A rich input editor stays a Phase 4 option, behind a flag, per shell profile. It
is not committed to.

## Consequences

- Every shell keeps its own line editing, including PSReadLine's history search
  and multiline handling. No reimplementation, no per-shell bug reports about
  editing.
- Typing latency is bounded by the shell's own redraw, plus our forward path.
- No syntax highlighting or Fig-spec completions. These depend on the editor, so
  they arrive with it or not at all.
- Commands are a string plus its exit status, not a structured object. Anything
  downstream (tasks, timeline, rerun) works from the text, which is what the
  `tf` CLI and the store already assume.
- If Phase 4 proceeds, it must be per-profile. A user who opts in for bash and
  opts out for PowerShell must get both behaviours, which means the editor has
  to be swappable at the session level rather than global.
- Interaction with `tf-input` matters more here than in a Warp-style build:
  bracketed paste, DECCKM and the Kitty keyboard protocol all exist to stay
  transparent to the shell's editor. `tf-input` must not "improve" input in ways
  the shell cannot see.