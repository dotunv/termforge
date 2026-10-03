# ADR 0009: Rendering the grid

- Status: Accepted
- Date: 2026-10-03
- Amends: build plan §5 ADR-9

## Context

The terminal grid has to look crisp and must not tear while doing it. Windows
adds a specific hazard the plan calls out: the in-box conhost *strips* images,
so the bundled ConPTY (ADR 0003) is a prerequisite for any graphics protocol
rather than merely a performance choice.

Text rendering is GPUI's DirectWrite-backed text system on Windows, which gives
font fallback and shaping without writing a text stack.

## Decision

**Text.** GPUI's text system. Ligatures are disabled by default for the
terminal grid (`FontFeatures::disable_ligatures()` in `bins/termforge/src/fonts.rs`)
and exposed as a setting, because in a terminal a ligature is a silent
misalignment between what is drawn and what bytes the cursor addresses. UI chrome
is unaffected.

**Font fallback.** Not yet wired: `fallbacks: None`. Nerd Font symbols and CJK
rendering therefore depend on the configured font covering them. Fallback chains
are needed before the IME and CJK test suites in Phase 4.

**Box drawing.** Drawn procedurally, not taken from the font, so adjacent cells
join without seams regardless of font metrics or size.

**Synchronized output.** DEC mode 2026 is parsed and exposed through
`tf_engine::Modes`, but the renderer does not yet batch to it. Tearing is
currently possible on fast output.

**Images.** Kitty graphics protocol and Sixel are deferred. Not a Phase 1 item,
and they need the bundled ConPTY to be present at all.

## Consequences

- Disabling ligatures keeps cursor addressing honest. This is a correctness
  choice, not taste.
- Procedural box drawing means the renderer must own those glyphs rather than
  asking the font. It also means cell metrics and the procedural strokes have to
  agree, which is a common source of visual seams.
- Exposing mode 2026 without honouring it is a half-feature. The engine work is
  done; the batching is not, and it should be finished before claiming tearing
  is fixed.
- Font fallback is a known gap, not a decision. Until `fallbacks` is populated,
  CJK and Nerd Font content will show as missing glyphs.
- Bundled ConPTY is a hard dependency for graphics support, which raises the
  stakes on ADR 0003's Spike A: without it, images are silently stripped rather
  than visibly rejected.