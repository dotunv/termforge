# TermForge

A Windows-first, GPU-rendered terminal workspace where every repo is a workspace: shells, services, agents and history, organised per project.

> Status: Phase 0 (foundation). Not usable as a daily terminal yet. The previous Go/Electron and native prototypes are preserved under the `archive/go-electron` and `archive/native-v0` tags.

## Architecture

```
bins/
  termforge   GPUI desktop app (UI process)
  forged      session host: owns PTYs and the store, survives UI restarts
  tf          CLI companion (notifications, shell integration)
crates/
  tf-proto    versioned IPC messages + length-prefixed postcard framing
  tf-tap      streaming OSC tap (133 marks, 7 / 9;9 cwd, 9 / 777 / 99 notifications)
  tf-engine   TerminalEngine trait + alacritty_terminal implementation
  tf-pty      portable-pty wrapper, ConPTY sideloading, shell discovery
  tf-session  PTY + tap + engine + virtual block index
  tf-store    SQLite (WAL) with versioned migrations
  tf-shell    shell integration scripts (pwsh, bash, zsh, fish) + injection
  tf-ui       framework-free design tokens and OKLCH theme generator
xtask/        cargo xtask conpty | ci
docs/adr/     architecture decision records
```

Data flows one way: PTY bytes -> `tf-tap` (observes, never rewrites) -> `tf-engine` (grid) -> UI. Marks are anchored to absolute grid lines, so blocks are an overlay on a single real terminal rather than separate buffers.

Key decisions are recorded in [`docs/adr`](docs/adr):

1. [UI framework: GPUI behind a facade](docs/adr/0001-ui-framework.md)
2. [Terminal engine: alacritty_terminal behind a trait](docs/adr/0002-terminal-engine.md)
3. [PTY and bundled ConPTY](docs/adr/0003-pty-and-conpty.md)
4. [Virtual blocks from OSC 133](docs/adr/0004-virtual-blocks.md)
5. [Process model: UI + forged](docs/adr/0005-process-model.md)
6. [IPC security](docs/adr/0006-ipc-security.md)

## Building

Requires stable Rust (see `rust-toolchain.toml`).

```sh
cargo test                       # core crates, fast
cargo xtask ci                   # fmt + clippy -D warnings + tests, same as CI
cargo run -p termforge           # desktop app (GPUI; first build is slow)
cargo run -p forged -- doctor    # environment diagnostics
```

On Windows, fetch the pinned ConPTY build so the terminal doesn't depend on the in-box version:

```sh
cargo xtask conpty               # writes assets/conpty/{conpty.dll,OpenConsole.exe}
```

Linux builds of the app need `libxkbcommon-dev libxcb1-dev libfontconfig-dev libfreetype-dev libwayland-dev libvulkan-dev`.

## Quality bar

- `clippy -D warnings`, `rustfmt`, `cargo-deny` on every PR, on Windows and Linux.
- Parsers are property-tested and fuzzed (`fuzz/`, nightly workflow).
- `unsafe` is denied workspace-wide; any exception needs an ADR and a `// SAFETY:` comment.
- Performance budgets (enforced once the renderer lands): p50 input latency <= 8 ms, 60 fps at 4K, cold start <= 400 ms, idle memory <= 150 MB.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
