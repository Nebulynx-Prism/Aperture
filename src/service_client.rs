//! OpenReality Service IPC client.
//!
//! Communicates with the background `OpenRealityService` over the local Named Pipe
//! (`\\.\pipe\OpenRealityService`) to request high-privilege operations:
//! - Setting Realtime process priority for the streamer
//! - Disabling EcoQoS / process power throttling
//! - Registering OpenXR runtime
//! - Restarting OVRService

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::time::Duration;
use tracing::{debug, error, info, warn};

pub const PIPE_NAME: &str = r"\\.\pipe\PrismApertureService";

#[derive(Serialize)]
struct ServiceRequest {
    command: String,
    pid: Option<u32>,
    path: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ServiceResponse {
    pub status: String,
    pub message: String,
}

pub struct ServiceClient;

impl ServiceClient {
    /// Ping the OpenReality service to verify it is responsive.
    pub fn ping() -> Result<bool> {
        match Self::send_command("ping", None, None) {
            Ok(resp) => Ok(resp.status == "ok" && resp.message == "pong"),
            Err(_) => Ok(false),
        }
    }

    /// Request the background service to boost this process to Realtime priority.
    pub fn boost_priority(pid: u32) -> Result<String> {
        let resp = Self::send_command("use_realtime_priority", Some(pid), None)?;
        if resp.status == "ok" {
            Ok(resp.message)
        } else {
            anyhow::bail!("Service error: {}", resp.message)
        }
    }

    /// Request the background service to disable power throttling on this process.
    pub fn disable_throttling(pid: u32) -> Result<String> {
        let resp = Self::send_command("disable_process_throttling", Some(pid), None)?;
        if resp.status == "ok" {
            Ok(resp.message)
        } else {
            anyhow::bail!("Service error: {}", resp.message)
        }
    }

    /// Set the active OpenXR runtime in HKLM.
    pub fn set_openxr_runtime(json_path: &str) -> Result<String> {
        let resp = Self::send_command("set_openxr_runtime", None, Some(json_path.to_string()))?;
        if resp.status == "ok" {
            Ok(resp.message)
        } else {
            anyhow::bail!("Service error: {}", resp.message)
        }
    }

    /// Send a command to the service over the named pipe with timeout handling.
    fn send_command(command: &str, pid: Option<u32>, path: Option<String>) -> Result<ServiceResponse> {
        let req = ServiceRequest {
            command: command.to_string(),
            pid,
            path,
        };
        let req_json = serde_json::to_string(&req)?;

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(PIPE_NAME)
            .context("Failed to open named pipe (OpenRealityService is not running)")?;

        file.write_all(req_json.as_bytes())
            .context("Failed to write to named pipe")?;
        file.flush()?;

        let mut buf = [0u8; 4096];
        let n = file.read(&mut buf).context("Failed to read from named pipe")?;
        if n == 0 {
            anyhow::bail!("Empty response from service");
        }

        let resp: ServiceResponse = serde_json::from_slice(&buf[..n])
            .context("Failed to parse service JSON response")?;

        Ok(resp)
    }
}
