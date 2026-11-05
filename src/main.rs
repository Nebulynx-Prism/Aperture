//! Prism Aperture Server — High-Performance VR Desktop Streamer
//!
//! Captures the Windows desktop, encodes video with hardware acceleration,
//! and streams it to Meta Quest headsets over local network or USB.

mod capture;
mod encoder;
mod audio;
mod network;
mod input;
mod settings;
mod service_client;
mod driver_bridge;
mod session;
mod ui;

use anyhow::Result;
use tracing_subscriber::{fmt, EnvFilter};

fn main() -> Result<()> {
    // Initialize logging
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Prism Aperture Server v{}", env!("CARGO_PKG_VERSION"));

    // Load settings
    let settings = settings::StreamerSettings::load_or_default();
    tracing::info!("Settings loaded: {:?}", settings.preferred_codec);

    // Launch the UI (which orchestrates everything)
    ui::run(settings)
}
