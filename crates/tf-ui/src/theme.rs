//! Theme generation from three inputs: base colour, accent, contrast.
//!
//! Every surface and text colour is derived in OKLCH so steps are
//! perceptually even in both light and dark themes. Text colours are then
//! nudged until they meet WCAG targets against the background.

use crate::color::{Oklch, Rgb};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeInput {
    pub base: Rgb,
    pub accent: Rgb,
    /// 0 = soft, 100 = maximum contrast.
    pub contrast: u8,
}

impl ThemeInput {
    /// The default dark theme.
    pub const DARK: Self = Self {
        base: Rgb::new(0x0f, 0x10, 0x11),
        accent: Rgb::new(0x5e, 0x6a, 0xd2),
        contrast: 30,
    };
    /// The default light theme.
    pub const LIGHT: Self = Self {
        base: Rgb::new(0xfb, 0xfb, 0xfc),
        accent: Rgb::new(0x5e, 0x6a, 0xd2),
        contrast: 30,
    };
}

/// Semantic colours consumed by the UI. Never reference raw colours in UI
/// code; add a semantic slot here instead.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub is_dark: bool,
    pub bg_app: Rgb,
    pub bg_panel: Rgb,
    pub bg_elevated: Rgb,
    pub bg_hover: Rgb,
    pub bg_selected: Rgb,
    pub border: Rgb,
    pub border_strong: Rgb,
    pub text: Rgb,
    pub text_muted: Rgb,
    pub text_faint: Rgb,
    pub accent: Rgb,
    pub accent_text: Rgb,
    pub focus_ring: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub danger: Rgb,
    /// Terminal background (matches the panel so the terminal feels native).
    pub term_bg: Rgb,
    pub term_fg: Rgb,
    pub term_cursor: Rgb,
    pub term_selection: Rgb,
    /// ANSI 0..15.
    pub ansi: [Rgb; 16],
}

impl Theme {
    pub fn generate(input: ThemeInput) -> Self {
        let base = Oklch::from_rgb(input.base);
        let accent = Oklch::from_rgb(input.accent);
        let is_dark = base.l < 0.6;
        let k = input.contrast.min(100) as f64 / 100.0;
        // Direction towards "more foreground".
        let dir = if is_dark { 1.0 } else { -1.0 };
        let chroma = base.c.min(0.03);
        let surface = |step: f64| base.with_c(chroma).with_l(base.l + dir * step * (1.0 + k));

        let bg_app = surface(0.0).to_rgb();
        let bg_panel = surface(0.012).to_rgb();
        let bg_elevated = surface(0.03).to_rgb();
        let bg_hover = surface(0.045).to_rgb();
        let bg_selected = surface(0.07).to_rgb();
        let border = surface(0.08).to_rgb();
        let border_strong = surface(0.14).to_rgb();

        let text_l = if is_dark {
            0.93 + 0.06 * k
        } else {
            0.22 - 0.12 * k
        };
        let text = ensure_contrast(
            base.with_c(chroma * 0.5).with_l(text_l),
            bg_app,
            7.0 + 5.0 * k,
            is_dark,
        );
        let text_muted = ensure_contrast(
            base.with_c(chroma)
                .with_l(if is_dark { 0.72 } else { 0.48 }),
            bg_app,
            4.5,
            is_dark,
        );
        let text_faint = ensure_contrast(
            base.with_c(chroma)
                .with_l(if is_dark { 0.55 } else { 0.62 }),
            bg_app,
            3.0,
            is_dark,
        );

        let accent_rgb = accent.to_rgb();
        let accent_text = if accent_rgb.contrast(Rgb::new(255, 255, 255)) >= 4.5 {
            Rgb::new(255, 255, 255)
        } else {
            Rgb::new(0, 0, 0)
        };

        let status = |h: f64| {
            ensure_contrast(
                Oklch::new(if is_dark { 0.72 } else { 0.55 }, 0.15, h),
                bg_panel,
                3.0,
                is_dark,
            )
        };

        let ansi = ansi_palette(is_dark, bg_panel);

        Self {
            is_dark,
            bg_app,
            bg_panel,
            bg_elevated,
            bg_hover,
            bg_selected,
            border,
            border_strong,
            text,
            text_muted,
            text_faint,
            accent: accent_rgb,
            accent_text,
            focus_ring: accent.with_l(if is_dark { 0.7 } else { 0.55 }).to_rgb(),
            success: status(150.0),
            warning: status(80.0),
            danger: status(25.0),
            term_bg: bg_panel,
            term_fg: text,
            term_cursor: accent.with_l(if is_dark { 0.8 } else { 0.5 }).to_rgb(),
            term_selection: accent
                .with_l(if is_dark { 0.35 } else { 0.88 })
                .with_c(0.08)
                .to_rgb(),
            ansi,
        }
    }
}

impl Theme {
    /// Resolve an xterm 256-colour index: 0..=15 from the theme, 16..=231
    /// the 6x6x6 cube, 232..=255 the grey ramp.
    pub fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb::new(level(i / 36), level((i / 6) % 6), level(i % 6))
            }
            232..=255 => {
                let v = 8 + (i - 232) * 10;
                Rgb::new(v, v, v)
            }
        }
    }
}

/// Step lightness away from `bg` until `target` contrast is reached.
fn ensure_contrast(mut c: Oklch, bg: Rgb, target: f64, is_dark: bool) -> Rgb {
    let dir = if is_dark { 0.01 } else { -0.01 };
    for _ in 0..100 {
        let rgb = c.to_rgb();
        if rgb.contrast(bg) >= target || c.l <= 0.0 || c.l >= 1.0 {
            return rgb;
        }
        c = c.with_l(c.l + dir);
    }
    c.to_rgb()
}

fn ansi_palette(is_dark: bool, bg: Rgb) -> [Rgb; 16] {
    // Hues: red, green, yellow, blue, magenta, cyan.
    const HUES: [f64; 6] = [25.0, 145.0, 90.0, 260.0, 320.0, 200.0];
    let (normal_l, bright_l) = if is_dark { (0.70, 0.80) } else { (0.50, 0.42) };
    let mut out = [Rgb::new(0, 0, 0); 16];
    let (black, white) = if is_dark { (0.30, 0.85) } else { (0.25, 0.92) };
    out[0] = Oklch::new(black, 0.0, 0.0).to_rgb();
    out[7] = Oklch::new(white, 0.0, 0.0).to_rgb();
    out[8] = Oklch::new(black + 0.15, 0.0, 0.0).to_rgb();
    out[15] = Oklch::new((white + 0.1).min(1.0), 0.0, 0.0).to_rgb();
    for (i, h) in HUES.iter().enumerate() {
        out[1 + i] = ensure_contrast(Oklch::new(normal_l, 0.14, *h), bg, 3.0, is_dark);
        out[9 + i] = ensure_contrast(Oklch::new(bright_l, 0.16, *h), bg, 3.0, is_dark);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(input: ThemeInput) {
        let t = Theme::generate(input);
        assert!(
            t.text.contrast(t.bg_app) >= 7.0,
            "text {:.2}",
            t.text.contrast(t.bg_app)
        );
        assert!(t.text_muted.contrast(t.bg_app) >= 4.5);
        assert!(t.text_faint.contrast(t.bg_app) >= 3.0);
        assert!(t.accent.contrast(t.accent_text) >= 3.0);
        for (i, c) in t
            .ansi
            .iter()
            .enumerate()
            .filter(|(i, _)| ![0, 7, 8, 15].contains(i))
        {
            assert!(c.contrast(t.term_bg) >= 3.0, "ansi {i} low contrast");
        }
        // Surfaces must be ordered monotonically away from the base.
        let l = |c: Rgb| Oklch::from_rgb(c).l;
        let steps = [
            t.bg_app,
            t.bg_panel,
            t.bg_elevated,
            t.bg_hover,
            t.bg_selected,
        ];
        for w in steps.windows(2) {
            if t.is_dark {
                assert!(l(w[1]) >= l(w[0]));
            } else {
                assert!(l(w[1]) <= l(w[0]));
            }
        }
    }

    #[test]
    fn indexed_palette_matches_xterm() {
        let t = Theme::generate(ThemeInput::DARK);
        assert_eq!(t.indexed(3), t.ansi[3]);
        assert_eq!(t.indexed(16), Rgb::new(0, 0, 0));
        assert_eq!(t.indexed(196), Rgb::new(255, 0, 0));
        assert_eq!(t.indexed(231), Rgb::new(255, 255, 255));
        assert_eq!(t.indexed(232), Rgb::new(8, 8, 8));
        assert_eq!(t.indexed(255), Rgb::new(238, 238, 238));
    }

    #[test]
    fn defaults_meet_contrast_targets() {
        check(ThemeInput::DARK);
        check(ThemeInput::LIGHT);
    }

    #[test]
    fn contrast_extremes_and_odd_bases() {
        for base in [
            "#000000", "#ffffff", "#1e1e2e", "#002b36", "#fdf6e3", "#2d1b3d",
        ] {
            for contrast in [0, 50, 100] {
                check(ThemeInput {
                    base: Rgb::from_hex(base).unwrap(),
                    accent: Rgb::from_hex("#e5484d").unwrap(),
                    contrast,
                });
            }
        }
    }
}
