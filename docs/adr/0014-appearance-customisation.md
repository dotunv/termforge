# ADR 0014: Appearance customisation

- Status: Accepted
- Date: 2026-10-11
- Extends: ADR 0008 (storage and configuration)

## Context

Theme, font size and sidebar width were in-memory only and reset on every
launch. `tf-config` existed but nothing in the app read it. Users need to
choose themes and fonts, bring their own, and keep the choice.

## Decision

**Persistence.** The app reads and writes `config.toml` through `tf-config`
(synchronously: GPUI has no tokio runtime and the file is tiny). Saves go
through a temp file and rename. If the file exists but does not parse, the
error is shown and saving is disabled, so a hand-edit typo never replaces the
user's file with defaults.

**Themes.** `*.toml` files in `<config dir>/themes/`. The native format is
the generator's three inputs (`base`, `accent`, `contrast`) plus optional
`[colors]` overrides per semantic slot and a 16-entry `ansi` list. Alacritty
colour files are accepted and converted, so existing collections work. Theme
selection is by name (`ui.theme_name`); `ui.accent` and `ui.contrast` override
the selected theme. Themes are built with the existing OKLCH generator, so
unset slots keep their WCAG guarantees; a theme that misses a target is
still applied and the settings page lists what fell short.

**Fonts.** `.ttf`, `.otf` and `.ttc` files in `<config dir>/fonts/` are
registered with GPUI's text system at startup and on reload. Each file is
added on its own so a corrupt file cannot hide the rest. The terminal font
picker lists families whose `i` and `m` advance widths match, because the
platform exposes no monospace flag.

**Untrusted input.** Theme files are likely to be downloaded. Parsing never
panics, rejects malformed colours rather than guessing, and is size-limited
(64 KiB per theme, 32 MiB per font, bounded file counts). The theme parser
has a property test and a fuzz target.

**Where it lives.** `tf-ui` is unchanged and dependency-free. Theme file
parsing is in `tf-config` (behind its existing `tf-ui` feature) because it
already owns TOML and the config directory. `TERMFORGE_CONFIG_DIR` overrides
the directory for tests and portable installs.

## Consequences

- Saving rewrites `config.toml` from the typed struct, so comments in a
  hand-edited file are lost when a setting is changed from the UI.
- The app polls file metadata for the config, themes and fonts every two
  seconds and reloads on change (skipped while a picker is previewing). This
  avoids a tokio runtime and a new dependency; `ConfigHandle`'s notify-based
  watcher remains available if the UI gains an async bridge.
- GPUI cannot unregister fonts, so replacing an already-loaded font file
  needs a restart.
- `terminal.ligatures` now defaults to `false`, matching what the grid
  always did; the previous default of `true` was never read.
- Window opacity and cursor blink remain in `config.toml` but are not wired
  to the UI.
