use anyhow::Result;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("TermForge starting — Phase 1 (libterm headless)");

    // Phase 2: create Win32 window, DX12 context, compositor, and run event loop.
    println!("TermForge — build Phase 1 complete. Run `cargo test -p libterm` to verify.");
    Ok(())
}
