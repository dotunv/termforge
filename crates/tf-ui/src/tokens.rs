//! Non-colour design tokens. Values are in logical pixels / milliseconds.

/// 4px spacing grid.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

pub mod radius {
    pub const SM: f32 = 4.0;
    pub const MD: f32 = 6.0;
    pub const LG: f32 = 8.0;
    pub const XL: f32 = 12.0;
}

/// Type scale for UI chrome (the terminal font is user-configured).
pub mod text {
    pub const XS: f32 = 11.0;
    pub const SM: f32 = 12.0;
    pub const MD: f32 = 13.0;
    pub const LG: f32 = 15.0;
    pub const XL: f32 = 18.0;
    pub const UI_FONT: &str = "Inter";
    pub const MONO_FONT: &str = "JetBrains Mono";
}

/// Motion: short and purposeful. Everything respects reduced-motion.
pub mod motion {
    pub const FAST_MS: u32 = 90;
    pub const BASE_MS: u32 = 140;
    pub const SLOW_MS: u32 = 220;
}

/// Layout sizes.
pub mod layout {
    pub const TITLEBAR_H: f32 = 36.0;
    pub const TAB_H: f32 = 30.0;
    pub const SIDEBAR_W: f32 = 240.0;
    pub const PALETTE_W: f32 = 640.0;
    pub const STATUSBAR_H: f32 = 24.0;
}
