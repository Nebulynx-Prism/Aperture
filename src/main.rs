//! Prism Aperture Server — High-Performance VR Desktop Streamer
//!
//! Captures the Windows desktop, encodes video with hardware acceleration,
//! and streams it to Meta Quest headsets over local network or USB.

mod audio;
mod capture;
mod cursor;
mod driver_bridge;
mod encoder;
mod input;
mod network;
mod service_client;
mod session;
mod settings;
mod ui;

use anyhow::Result;
use tracing_subscriber::{fmt, EnvFilter};

fn main() -> Result<()> {
    // Initialize logging
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Prism Aperture Server v{}", env!("CARGO_PKG_VERSION"));

    // Load settings
    let settings = settings::StreamerSettings::load_or_default();
    tracing::info!("Settings loaded: {:?}", settings.preferred_codec);

    // Console mode also provides a bounded smoke-test path without opening UI.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--configure-pcvr") {
        let manifest = driver_bridge::configure_runtime()?;
        println!(
            "Prism driver registered. OpenXR runtime manifest: {}",
            manifest.display()
        );
        if !args.iter().any(|arg| arg == "--headless") {
            return Ok(());
        }
    }
    if args.iter().any(|arg| arg == "--pcvr") {
        std::env::set_var("PRISM_PCVR_AUTO", "1");
    }
    if args.iter().any(|arg| arg == "--headless") {
        let seconds = args
            .windows(2)
            .find(|pair| pair[0] == "--run-seconds")
            .map(|pair| pair[1].parse::<u64>())
            .transpose()?;
        let session = session::start_session(settings);
        let started = std::time::Instant::now();
        while seconds.map_or(true, |limit| started.elapsed().as_secs() < limit) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        session.stop();
        std::thread::sleep(std::time::Duration::from_millis(500));
        return Ok(());
    }

    // Launch the UI (which orchestrates everything)
    ui::run(settings)
}
