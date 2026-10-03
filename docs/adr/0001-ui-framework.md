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

## Amendment: the facade boundary is tighter than planned

- Date: 2026-10-03
- Status: Accepted

The build plan (§5, ADR-1 mitigation) proposed keeping `tf-ui` as *the* crate that
imports `gpui`, with GPUI-facing components living there. That turned out to be the
wrong shape once there was real code to place.

What actually happened:

- `tf-ui` holds design tokens, the OKLCH theme generator and colour resolution. It
  has **no** GPUI dependency and is testable in milliseconds.
- `bins/termforge` is the only crate that imports `gpui`. It also owns the terminal
  view and grid painting.
- Selection, search and link parsing live in `tf_engine::text`, which is
  framework-free and unit-tested without a window.

This inverts the plan's containment: instead of `tf-ui` being the GPUI boundary, the
*binary* is. The reasoning that still holds is unchanged — one place owns the
framework — but "one crate" became "one binary", and the framework-free logic ended
up in the engine crate rather than the facade.

The cost is that `terminal_view.rs` and `paint.rs` are large and cannot be tested
without a GPUI test context. Mitigations in place: painting is a pure function of a
snapshot plus overlays, so it is at least separable from GPUI, and all text handling
lives in `tf_engine::text`.

Revisit if the app gains a second window or a component gallery binary: at that
point a real `tf-term-view` crate with a GPUI-facing facade earns its keep.
