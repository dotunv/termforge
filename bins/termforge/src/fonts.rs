//! Monospace font selection.

use gpui::{App, Font, FontFeatures, FontStyle, FontWeight, SharedString};

/// Preferred terminal fonts, best first. Cascadia ships with Windows
/// Terminal and Windows 11; Consolas ships with every supported Windows.
const PREFERRED: &[&str] = &[
    "Cascadia Mono",
    "Cascadia Code",
    "JetBrains Mono",
    "Consolas",
    "SF Mono",
    "Menlo",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
];

#[derive(Debug, Clone)]
pub struct MonoFont {
    pub family: SharedString,
}

impl MonoFont {
    pub fn detect(cx: &App) -> Self {
        let available = cx.text_system().all_font_names();
        let family = PREFERRED
            .iter()
            .find(|want| available.iter().any(|have| have.eq_ignore_ascii_case(want)))
            .copied()
            .unwrap_or("monospace");
        tracing::info!(font = family, "terminal font");
        Self {
            family: family.into(),
        }
    }

    pub fn regular(&self) -> Font {
        Font {
            family: self.family.clone(),
            // Terminals want fixed-width digits and no ligatures by default.
            features: FontFeatures::disable_ligatures(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }
}
