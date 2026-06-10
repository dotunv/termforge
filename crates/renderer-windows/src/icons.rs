//! Semantic icon glyphs from Windows' system icon font (Segoe Fluent Icons on
//! Windows 11, Segoe MDL2 Assets on Windows 10).  These are Private-Use-Area
//! codepoints shared between both fonts, so they render identically without
//! shipping a bundled TTF.  Drawn via [`crate::ui_renderer::UiCommand::DrawIcon`].

/// Window caption — minimize.
pub const MINIMIZE: &str = "\u{E921}";
/// Window caption — maximize.
pub const MAXIMIZE: &str = "\u{E922}";
/// Window caption — restore (maximised → windowed).
pub const RESTORE: &str = "\u{E923}";
/// Window caption — close.
pub const CLOSE: &str = "\u{E8BB}";

/// Settings gear.
pub const SETTINGS: &str = "\u{E713}";
/// Plus / new.
pub const ADD: &str = "\u{E710}";
/// Overflow / more (…).
pub const MORE: &str = "\u{E712}";
/// Search / find.
pub const SEARCH: &str = "\u{E721}";

/// Disclosure chevrons.
pub const CHEVRON_RIGHT: &str = "\u{E76C}";
pub const CHEVRON_DOWN: &str = "\u{E70D}";

/// Workspace / folder.
pub const FOLDER: &str = "\u{E8B7}";
/// Local shell / command prompt.
pub const TERMINAL: &str = "\u{E756}";
/// Remote / SSH (globe).
pub const GLOBE: &str = "\u{E774}";
/// Key vault / lock.
pub const LOCK: &str = "\u{E72E}";

/// Status — success / check.
pub const CHECK: &str = "\u{E73E}";
/// Status — cancel / error mark.
pub const CANCEL: &str = "\u{E711}";
/// Recent / clock (durations, history).
pub const CLOCK: &str = "\u{E823}";
/// Notification bell.
pub const BELL: &str = "\u{E7ED}";
/// Sync / running (use with a spinner animation if desired).
pub const SYNC: &str = "\u{E895}";
