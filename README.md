# TermForge

A Windows-first, GPU-rendered terminal workspace where every repo is a workspace: shells, services, agents and history, organised per project.

> Status: Phase 2 foundation. One GPU-rendered terminal per window with shell integration, command blocks and OSC 7501 program status. Selection, search, links and mouse reporting work. `forged` owns authenticated sessions, bounded output replay, and durable project tasks with context summaries. The UI reconnects to a hosted session for the current canonical project and can start the bundled daemon automatically. Project shells, their latest working directories, dimensions, and task association are recreated after a daemon restart. Not yet a daily driver: integrated task UI, tabs, splits and settings are unfinished. The previous Go/Electron and native prototypes are preserved under the `archive/go-electron` and `archive/native-v0` tags.

## Architecture

```
bins/
  termforge   GPUI desktop app (UI process)
  forged      session host: owns PTYs and the store, survives UI restarts
  tf          CLI companion (notifications, shell integration)
crates/
  tf-proto    versioned IPC messages + length-prefixed postcard framing
  tf-tap      streaming OSC tap (133 marks, 7501 program status, cwd, progress, notifications)
  tf-engine   TerminalEngine trait + alacritty_terminal implementation (cells, colours, modes, scrollback)
  tf-input    key and paste encoding (xterm conventions, safe bracketed paste)
  tf-pty      portable-pty wrapper, ConPTY sideloading, shell discovery
  tf-session  PTY + tap + engine + virtual block index; LiveSession runs it on background threads
  tf-store    SQLite (WAL) with versioned migrations
  tf-shell    shell integration scripts (pwsh, bash, zsh, fish) + injection
  tf-ui       framework-free design tokens and OKLCH theme generator
  tf-update   signed update-manifest verification (no I/O)
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
7. [Input editor: defer to the shell](docs/adr/0007-input-editor.md)
8. [Storage and config](docs/adr/0008-storage-and-config.md)
9. [Rendering the grid](docs/adr/0009-rendering.md)
10. [Program status (OSC 7501)](docs/adr/0010-program-status.md)
11. [Remote workspaces](docs/adr/0011-remote-workspaces.md)
12. [Split panes](docs/adr/0012-split-panes.md) (proposed)
13. [Signed updates](docs/adr/0013-signed-updates.md)

## Building

Requires stable Rust (see `rust-toolchain.toml`).

```sh
cargo test                       # core crates, fast
cargo xtask ci                   # fmt + clippy -D warnings + tests, same as CI
cargo run -p termforge           # desktop app (GPUI; first build is slow)
cargo run -p forged -- doctor    # environment diagnostics
cargo xtask spike-a              # slow ConPTY ordering harness; see below
cargo xtask dist                 # native beta bundle in target/dist
```

Tagged beta builds and the required native smoke checklist are documented in
[`docs/releasing.md`](docs/releasing.md).

### Spike A: ConPTY ordering

OSC 133 marks must stay ordered relative to output, or command blocks, exit
statuses and cwd tracking are all subtly wrong. Warp had to fork ConPTY after
finding it emitted OSC out of order, so this is checked against real shells over
a long run rather than trusted.

`cargo xtask ci` runs a short version (12 commands per shell) on every PR. The
full 10,000-command run is too slow for the PR matrix and lives in the
[Spike A workflow](.github/workflows/spike-a.yml), which runs on `main`,
nightly, and on demand. Shells that aren't installed skip themselves; the job
fails if no supported shell was available, so a green run always means real
coverage.

```sh
cargo xtask spike-a --commands 20000 --shells Bash,Pwsh
```

On Windows, fetch the pinned ConPTY build so the terminal doesn't depend on the in-box version:

```sh
cargo xtask conpty               # writes assets/conpty/{conpty.dll,OpenConsole.exe}
```

Linux builds of the app need `libxkbcommon-dev libxcb1-dev libfontconfig-dev libfreetype-dev libwayland-dev libvulkan-dev`.

## Using the app

| Action | Shortcut |
|---|---|
| Select | drag; double-click for a word, triple-click for a line; `Shift`+click extends |
| Copy selection | `Ctrl+C` (only when something is selected), `Ctrl+Shift+C`, right-click |
| Paste | `Ctrl+Shift+V`, `Shift+Insert`, right-click with no selection |
| Select all | `Ctrl+Shift+A` |
| Find | `Ctrl+Shift+F`; `Enter`/`Up` older, `Shift+Enter`/`Down` newer, `Alt+C` match case, `Esc` close |
| Open link | `Ctrl`+click (http/https only) |
| Select in apps that use the mouse | hold `Shift` |
| Scroll back | mouse wheel, `Shift+PageUp` / `Shift+PageDown`, `Ctrl+Shift+Home` / `End` |
| Font size | `Ctrl+=`, `Ctrl+-`, `Ctrl+0` |
| Restart after exit | `Enter` |
| Command palette | `Ctrl+Shift+P`; type to filter, `Up`/`Down`, `Enter`, `Esc` |

Programs can report structured status with OSC 7501. The tab indicator and
status bar distinguish working, blocked, done and failed programs. Scripts can
emit valid reports without constructing escape sequences themselves:

```sh
tf status working --app cargo --message "Running tests"
tf status blocked --kind question --message "Choose a deployment target"
tf status done --message "Tests passed"
tf status clear
```

### Remote SSH preview

TermForge delegates SSH transport, host-key verification and authentication to
the installed OpenSSH client. Press `Ctrl+Shift+P` and choose `SSH: connect to
<host>` to connect to a concrete alias declared by a `Host` directive in
`~/.ssh/config`. Wildcard rules are applied by OpenSSH but omitted from the
picker because they are not concrete destinations.

For automation or aliases not listed in the main config file, start the current
workspace as an SSH connection with any host argument understood by `ssh`
(including Tailscale MagicDNS names):

```sh
TERMFORGE_SSH_HOST=devbox cargo run -p termforge
```

The SSH PTY is owned by `forged`, so it survives a UI restart. Remote working
directories remain tagged as remote and are never reused as local restart
directories. This preview has no host picker, automatic remote shell
integration or daemon-crash recovery yet; see ADR 0011.

The left gutter marks each command block: accent while running, red on a non-zero exit, neutral on success.

Blocks need shell integration, which is injected automatically for PowerShell,
Windows PowerShell, bash and Git Bash. For zsh and fish, add
`source (tf shell-integration fish | psub)` or the zsh equivalent to your rc
file.

`cmd.exe` and WSL are launchable but get no integration yet, so they show no
blocks, no exit status and no cwd tracking. WSL additionally needs the
integration script's path translated into the distro before injection can work.
`ShellProfile::supports_integration()` reports this; the UI should surface it
rather than let it look like a broken terminal.

## Quality bar

- `clippy -D warnings`, `rustfmt`, `cargo-deny` on every PR, on Windows and Linux.
- Parsers are property-tested and fuzzed (`fuzz/`, nightly workflow).
- `unsafe` is denied workspace-wide; any exception needs an ADR and a `// SAFETY:` comment.
- Performance budgets: p50 input latency <= 8 ms, 60 fps at 4K, cold start <= 400 ms, idle memory <= 150 MB.

### What is actually measured

"If it isn't measured, it doesn't count." Current state, honestly:

| Budget | Status |
|---|---|
| Keystroke -> visible output, p50 <= 8 ms | **Enforced**, `cargo xtask latency`. Covers PTY, tap, engine, snapshot. Does **not** cover GPUI paint or scanout. |
| OSC 133 ordering over 10,000 commands | **Enforced**, [slow gates](.github/workflows/spike-a.yml), on `main` and nightly. |
| 60 fps at 4K, frame time | Not measured. Needs a window and a display pipeline. |
| Cold start <= 400 ms | Not measured. |
| Idle memory <= 150 MB | Not measured. |
| VT conformance (`vttest`/`esctest`) | Not present. One `insta` snapshot in `tf-engine`; no golden-grid suite. |
| Real-app matrix (vim, htop, lazygit, PSReadLine, agents) | Not present. |
| Crash reporting | Not present. |
| Accessibility (AccessKit), IME | Not present. |

The latency probe is a necessary condition for the input budget, not the whole
of it: it stops at the snapshot, so a renderer regression would not show up
there. It needs its own frame-timestamp probe before the 8 ms claim covers the
full pipeline.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
