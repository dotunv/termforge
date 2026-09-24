//! Colour math: sRGB <-> OKLab/OKLCH and WCAG contrast.

/// 8-bit sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#rrggbb` or `rrggbb`.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 || !s.is_ascii() {
            return None;
        }
        let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        Some(Self::new(p(0)?, p(2)?, p(4)?))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// WCAG 2.x relative luminance.
    pub fn luminance(self) -> f64 {
        let f = |c: u8| to_linear(c as f64 / 255.0);
        0.2126 * f(self.r) + 0.7152 * f(self.g) + 0.0722 * f(self.b)
    }

    /// WCAG 2.x contrast ratio, 1.0..=21.0.
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

/// Perceptual colour: lightness 0..1, chroma 0..~0.4, hue in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

impl Oklch {
    pub const fn new(l: f64, c: f64, h: f64) -> Self {
        Self { l, c, h }
    }

    pub fn from_rgb(rgb: Rgb) -> Self {
        let r = to_linear(rgb.r as f64 / 255.0);
        let g = to_linear(rgb.g as f64 / 255.0);
        let b = to_linear(rgb.b as f64 / 255.0);
        let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
        let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
        let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
        let ol = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
        let oa = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
        let ob = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
        let c = (oa * oa + ob * ob).sqrt();
        let h = ob.atan2(oa).to_degrees().rem_euclid(360.0);
        Self { l: ol, c, h }
    }

    /// Convert to sRGB, reducing chroma until the colour is in gamut so hue
    /// and lightness are preserved.
    pub fn to_rgb(self) -> Rgb {
        let mut c = self.c;
        loop {
            if let Some(rgb) = self.with_c(c).to_rgb_exact() {
                return rgb;
            }
            if c <= 1e-4 {
                return self.with_c(0.0).to_rgb_exact().unwrap_or(Rgb::new(0, 0, 0));
            }
            c *= 0.95;
        }
    }

    fn to_rgb_exact(self) -> Option<Rgb> {
        let (a, b) = (
            self.c * self.h.to_radians().cos(),
            self.c * self.h.to_radians().sin(),
        );
        let l_ = self.l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
        let m_ = self.l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
        let s_ = self.l - 0.089_484_177_5 * a - 1.291_485_548 * b;
        let (l, m, s) = (l_.powi(3), m_.powi(3), s_.powi(3));
        let r = 4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s;
        let g = -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s;
        let bl = -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s;
        let ch = |v: f64| -> Option<u8> {
            const EPS: f64 = 1e-6;
            if !(-EPS..=1.0 + EPS).contains(&v) {
                return None;
            }
            Some((from_linear(v.clamp(0.0, 1.0)) * 255.0).round() as u8)
        };
        Some(Rgb::new(ch(r)?, ch(g)?, ch(bl)?))
    }

    pub fn with_l(self, l: f64) -> Self {
        Self {
            l: l.clamp(0.0, 1.0),
            ..self
        }
    }

    pub fn with_c(self, c: f64) -> Self {
        Self {
            c: c.max(0.0),
            ..self
        }
    }
}

fn to_linear(c: f64) -> f64 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = Rgb::from_hex("#5e6ad2").unwrap();
        assert_eq!(c.to_hex(), "#5e6ad2");
        assert!(Rgb::from_hex("#12345").is_none());
        assert!(Rgb::from_hex("#gg0000").is_none());
    }

    #[test]
    fn oklch_roundtrip_is_lossless_for_srgb() {
        for hex in [
            "#000000", "#ffffff", "#5e6ad2", "#ff0000", "#0f1011", "#e2e4e7",
        ] {
            let c = Rgb::from_hex(hex).unwrap();
            assert_eq!(Oklch::from_rgb(c).to_rgb(), c, "{hex}");
        }
    }

    #[test]
    fn contrast_extremes() {
        let (b, w) = (Rgb::new(0, 0, 0), Rgb::new(255, 255, 255));
        assert!((b.contrast(w) - 21.0).abs() < 0.01);
        assert!((w.contrast(w) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn out_of_gamut_is_mapped_not_clipped() {
        let c = Oklch::new(0.7, 0.4, 150.0).to_rgb();
        let back = Oklch::from_rgb(c);
        assert!((back.l - 0.7).abs() < 0.02);
        assert!((back.h - 150.0).abs() < 3.0);
    }
}
