//! Privacy-safe local crash breadcrumbs.
//!
//! Nothing is uploaded. The log intentionally excludes terminal output,
//! commands, environment variables and workspace state; a future opt-in
//! reporter can attach it only after showing the user exactly what will leave
//! the machine.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn install_local_crash_log(data_dir: Option<PathBuf>) {
    let Some(path) = data_dir.map(|dir| dir.join("crashes.log")) else {
        return;
    };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default();
            let location = info
                .location()
                .map(|location| format!("{}:{}", location.file(), location.line()))
                .unwrap_or_else(|| "unknown".into());
            let _ = writeln!(
                file,
                "time={timestamp} version={} platform={}/{} location={location}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
            );
        }
        previous(info);
    }));
}
