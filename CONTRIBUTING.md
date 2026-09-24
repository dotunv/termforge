# Contributing

## Workflow

1. Open an issue or comment on one before large changes.
2. Branch from `main`; keep PRs focused (one concern, ideally < 400 lines).
3. Run `cargo xtask ci` before pushing. CI runs the same checks on Windows and Linux.
4. Use [Conventional Commits](https://www.conventionalcommits.org): `feat(tap): ...`, `fix(pty): ...`.

## Rules of the codebase

- **Dependencies flow downward.** `tf-proto`, `tf-tap`, `tf-ui` depend on nothing internal. Only `bins/termforge` may depend on `gpui`.
- **No raw colours in UI code.** Add a semantic slot to `tf_ui::Theme`.
- **Bytes, not strings, across process and PTY boundaries.** Decode only at the edge that renders.
- **Every parser gets a property test**, and a fuzz target if it reads untrusted input.
- **Architectural changes need an ADR** in `docs/adr/` (copy the format of an existing one).
- **No `unwrap()` outside tests.** Use `?` with context, or document why a panic is impossible with `expect`.

## Tests

- Unit tests live next to the code; cross-crate tests in `crates/*/tests/`.
- Tests that spawn real shells must skip cleanly when that shell is missing.
- Snapshot tests use `insta`; review changes with `cargo insta review`.
