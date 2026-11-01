# Architectural Specification: TermForge Blender-Style Workspace Engine

## 1. Core Vision & Problem Statement
Current Status:
TermForge currently follows a fixed IDE shell layout (static left sidebar, hardcoded dashboard summary cards in `workbench.rs`, and a fixed bottom PTY terminal strip).

Target State:
Transition TermForge into a Blender-inspired spatial workspace engine.
In Blender:
- The screen has NO permanent sidebars, headers, or fixed panels.
- Every screen section is a generic "Area" that can host any "Editor Type".
- Areas can be split horizontally/vertically or joined by dragging borders/corners.
- Workspaces in the top bar are preset configurations of the split-pane binary tree, not sub-page document tabs.
- The center surface is an interactive visual canvas/viewport rather than static web-style cards.

---

## 2. Immediate Fix: Protocol Handshake Synchronization
- Issue: Client error `connect to forged: protocol error: expected welcome, got Response { id: 0, result: Err(VersionMismatch { server: 9, client: 10 }) }`.
- Action:
  1. Inspect `crates/tf-proto/src/lib.rs` and align the protocol version constant between `bins/termforge/` and `bins/forged/`.
  2. Ensure clean builds across both binaries so IPC daemon communication works reliably.
  3. Decouple UI rendering from daemon connection failures: if `forged` is offline or mismatched, display a non-blocking status indicator in the bottom status bar rather than breaking or freezing the workspace.

---

## 3. Pillar 1: Generic Area Model (`bins/termforge/src/panes.rs` & `split.rs`)
Replace the terminal-specific split tree with a polymorphic area container:

1. Define an editor kind enum:
   ```rust
   pub enum EditorKind {
       Terminal(PtySessionId),
       GraphCanvas(CanvasState),
       Inspector(InspectorState),
       Outliner(ProjectTreeState),
       Timeline(LogStreamState),
   }
   ```

2. Refactor `AreaNode` in `panes.rs`:
Each leaf in the binary split tree holds:
* A unique `AreaId`.
* The active `EditorKind`.
* A standardized header strip containing:
  * Editor Type Selector: A compact dropdown on the top-left to swap the editor type in place (Terminal <-> Canvas <-> Inspector).
  * Context Actions: Specific toolbar buttons for the mounted editor.
  * Split/Maximize/Close buttons.

3. Keep boundary dragging and splitting capabilities in `split.rs`, making sure splitting an area preserves its neighbor geometry and assigns a default editor to the new split.

---

## 4. Pillar 2: Workspace Presets as Split Trees (`bins/termforge/src/workspace.rs` & `chrome.rs`)

Redefine the top navigation tabs:

* Replace static page routes (`Project`, `Files`, `Git`, `Tasks`, `Terminal`) with Layout Presets:
  * **Topology / Flow:** 70% Canvas on the left, 30% Right split (Inspector top, mini-terminal bottom).
  * **Console / Multi-Terminal:** 2x2 or 3-column full-bleed split terminal matrix.
  * **State / Debug:** 50% Timeline log waterfall + 50% State/Config Inspector.

* Session Lifecycle:
  * Switching workspace tabs must NEVER kill active PTY processes or `forged` background sessions.
  * Sessions must live in a persistent session registry (`crates/tf-session/`); layout changes simply remount active session handles into whichever areas specify `EditorKind::Terminal`.

---

## 5. Pillar 3: Central Interactive Viewport (`bins/termforge/src/terminal_view/workbench.rs` & `surface.rs`)

1. Deprecate the static dashboard cards (`Tasks: Unavailable`, `Sessions: 1`, `Changed Files: 30`) in `workbench.rs`.
2. Implement a 2D pan/zoom canvas (`GraphCanvas`):
   * Grid rendering with infinite canvas coordinate transformations (pan & zoom matrix).
   * Render actionable system nodes:
     * Background Daemons / Workers (`forged`).
     * Git worktrees / branches (`main`, `e0f8/termforge`).
     * Docker containers or execution targets.
   * Node Interaction:
     * Selecting a node directs its properties into any visible `Inspector` area.
     * Selecting a node routes its streaming I/O or PTY attach into any visible `Terminal` area.

---

## 6. Implementation Milestones for Antigravity

1. **Milestone A:** Resolve the `tf-proto` protocol version mismatch and verify IPC communication between `termforge` and `forged`.
2. **Milestone B:** Introduce `EditorKind` and the area header dropdown in `panes.rs`.
3. **Milestone C:** Wire layout presets into `workspace.rs` so switching workspaces swaps split trees without killing sessions.
4. **Milestone D:** Replace the static dashboard cards in `workbench.rs` with the interactive canvas surface.

