# ADR 0003: portable-pty with a bundled ConPTY

- Status: Accepted
- Date: 2026-09-24

## Context

The in-box ConPTY in `kernel32.dll` varies with the Windows build and has had long-standing bugs (dropped or reordered output, resize artefacts, OSC passthrough issues). Windows Terminal, WezTerm and Rio ship Microsoft's out-of-band ConPTY instead. `portable-pty` 0.9 already prefers a sideloaded `conpty.dll` over the in-box one.

The sideloaded `conpty.dll` needs `OpenConsole.exe` next to it; if only the DLL is present it silently falls back to in-box behaviour.

## Decision

- Use `portable-pty` 0.9 in `tf-pty`.
- Pin `Microsoft.Windows.Console.ConPTY` 1.24.260710001 from NuGet. `cargo xtask conpty` downloads it, verifies its SHA-256 and extracts both files to `assets/conpty/`.
- Release bundles place `conpty.dll` and `OpenConsole.exe` next to `forged.exe`. `forged doctor` reports which ConPTY is active.

## Consequences

- Behaviour is consistent across Windows 10 and 11 builds.
- Binaries under `assets/conpty/` are never committed; they are fetched and verified.
- Upgrading ConPTY means changing the version and hash in `xtask` and re-running the ordering harness.
