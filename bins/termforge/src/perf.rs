//! Lightweight production-safe renderer instrumentation.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static FIRST_PAINT: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static PAINT_NANOS: AtomicU64 = AtomicU64::new(0);
static MAX_PAINT_NANOS: AtomicU64 = AtomicU64::new(0);

pub fn mark_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

pub fn record_grid_paint(elapsed: Duration) {
    if !FIRST_PAINT.swap(true, Ordering::Relaxed) {
        let cold_start = PROCESS_START.get().map(Instant::elapsed);
        tracing::info!(
            ?cold_start,
            rss_bytes = resident_memory_bytes(),
            "first terminal frame painted"
        );
    }
    let nanos = elapsed.as_nanos().min(u64::MAX as u128) as u64;
    let count = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    PAINT_NANOS.fetch_add(nanos, Ordering::Relaxed);
    MAX_PAINT_NANOS.fetch_max(nanos, Ordering::Relaxed);
    if count.is_multiple_of(120) {
        let total = PAINT_NANOS.swap(0, Ordering::Relaxed);
        let max = MAX_PAINT_NANOS.swap(0, Ordering::Relaxed);
        tracing::debug!(
            frames = 120,
            avg_paint_ms = total as f64 / 120.0 / 1_000_000.0,
            max_paint_ms = max as f64 / 1_000_000.0,
            rss_bytes = resident_memory_bytes(),
            "terminal paint metrics"
        );
    }
}

#[cfg(target_os = "linux")]
fn resident_memory_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    kb.checked_mul(1024)
}

#[cfg(not(target_os = "linux"))]
fn resident_memory_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rss_is_readable() {
        assert!(super::resident_memory_bytes().is_some());
    }
}
