# ADR 0001: UI framework is GPUI, behind a thin facade

- Status: Accepted
- Date: 2026-09-24

## Context

TermForge needs a GPU-rendered, keyboard-first desktop UI with the polish of Warp, Linear and Raycast, and it has to be excellent on Windows. Options considered: Electron/Tauri (web UI), egui/iced (immediate/Elm-style Rust UIs), Slint, and GPUI (Zed's framework, which Warp's open-source `warpui` also descends from conceptually).

- Web UIs make rich chrome easy but put the terminal grid behind a DOM or canvas bridge with measurable latency and memory cost.
- egui/iced are pleasant but lack the text shaping, layout and styling depth needed for a Linear-grade interface.
- GPUI renders through DirectX 11 on Windows, has production-grade text and layout, and powers a shipping editor. Its API still moves between releases.

## Decision

Use `gpui` pinned to an exact version (`=0.2.2`) and only in `bins/termforge`. All design decisions (tokens, themes, contrast rules) live in the framework-free `tf-ui` crate. Terminal state is produced by crates that never depend on GPUI.

## Consequences

- Upgrading GPUI is a deliberate, single-crate change.
- If GPUI becomes untenable, only the view layer is rewritten; engine, sessions, store and theme generation are untouched.
- First builds of the app are slow (large dependency graph); core crates stay fast to build and test.
- `gpui-ce`, `gpui-component` and Warp's `warpui` are tracked as references, not dependencies.
