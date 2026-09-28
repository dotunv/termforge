# TermForge

A Windows-first, GPU-rendered terminal workspace where every repo is a workspace: shells, services, agents and history, organised per project.

> Status: Phase 1 (terminal core). One GPU-rendered terminal per window with shell integration and command blocks. Not yet a daily driver: no tabs, splits, selection or `forged` IPC. The previous Go/Electron and native prototypes are preserved under the `archive/go-electron` and `archive/native-v0` tags.

## Architecture

```
bins/
  termforge   GPUI desktop app (UI process)
  forged      session host: owns PTYs and the store, survives UI restarts
  tf          CLI companion (notifications, shell integration)
crates/
  tf-proto    versioned IPC messages + length-prefixed postcard framing
  tf-tap      streaming OSC tap (133 marks, 7 / 9;9 cwd, 9 / 777 / 99 notifications)
  tf-engine   TerminalEngine trait + alacritty_terminal implementation (cells, colours, modes, scrollback)
  tf-input    key and paste encoding (xterm conventions, safe bracketed paste)
  tf-pty      portable-pty wrapper, ConPTY sideloading, shell discovery
  tf-session  PTY + tap + engine + virtual block index; LiveSession runs it on background threads
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

## Using the app

| Action | Shortcut |
|---|---|
| Paste | `Ctrl+Shift+V`, `Shift+Insert`, right-click |
| Copy visible screen | `Ctrl+Shift+C` (selection comes in Phase 2) |
| Scroll back | mouse wheel, `Shift+PageUp` / `Shift+PageDown` |
| Font size | `Ctrl+=`, `Ctrl+-`, `Ctrl+0` |
| Restart after exit | `Enter` |

The left gutter marks each command block: accent while running, red on a non-zero exit, neutral on success. Blocks need shell integration, which is injected automatically for PowerShell, Windows PowerShell, bash and Git Bash. For zsh and fish, add `source (tf shell-integration fish | psub)` or the zsh equivalent to your rc file.

## Quality bar

- `clippy -D warnings`, `rustfmt`, `cargo-deny` on every PR, on Windows and Linux.
- Parsers are property-tested and fuzzed (`fuzz/`, nightly workflow).
- `unsafe` is denied workspace-wide; any exception needs an ADR and a `// SAFETY:` comment.
- Performance budgets (enforced once the renderer lands): p50 input latency <= 8 ms, 60 fps at 4K, cold start <= 400 ms, idle memory <= 150 MB.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
