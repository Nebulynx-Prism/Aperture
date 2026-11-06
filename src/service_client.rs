//! OpenReality Service IPC client.
//!
//! Communicates with the background `PrismApertureService` over the local Named Pipe
//! (`\\.\pipe\PrismApertureService`) to request high-privilege operations:
//! - Setting High process priority for the streamer
//! - Disabling EcoQoS / process power throttling
//! - Registering OpenXR runtime
//! - Restarting OVRService

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{ERROR_NO_DATA, ERROR_PIPE_BUSY, HANDLE};
use windows::Win32::System::Pipes::{SetNamedPipeHandleState, PIPE_NOWAIT, PIPE_READMODE_BYTE};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(5);

fn wait_for_pipe_progress(deadline: Instant) -> Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    anyhow::ensure!(!remaining.is_zero(), "Service IPC deadline exceeded");
    std::thread::sleep(POLL_INTERVAL.min(remaining));
    Ok(())
}

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

    /// Request the background service to boost this process to High priority.
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

    /// Send a bounded newline-framed command with a two-second transaction deadline.
    fn send_command(
        command: &str,
        pid: Option<u32>,
        path: Option<String>,
    ) -> Result<ServiceResponse> {
        let req = ServiceRequest {
            command: command.to_string(),
            pid,
            path,
        };
        let req_json = serde_json::to_string(&req)?;

        Self::exchange(PIPE_NAME, &req_json, COMMAND_TIMEOUT)
    }

    /// A single deadline covers pipe-busy retries, request writes and response reads.
    /// PIPE_NOWAIT keeps the synchronous public API without unbounded kernel I/O.
    fn exchange(pipe_name: &str, req_json: &str, timeout: Duration) -> Result<ServiceResponse> {
        anyhow::ensure!(req_json.len() <= 4096, "Service request exceeds 4096 bytes");
        let deadline = Instant::now() + timeout;
        let mut file = loop {
            anyhow::ensure!(Instant::now() < deadline, "Service IPC deadline exceeded");
            match OpenOptions::new().read(true).write(true).open(pipe_name) {
                Ok(file) => break file,
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) => {
                    wait_for_pipe_progress(deadline)?;
                }
                Err(error) => {
                    return Err(error)
                        .context("Failed to open named pipe (PrismApertureService is not running)")
                }
            }
        };
        unsafe {
            SetNamedPipeHandleState(
                HANDLE(file.as_raw_handle()),
                Some(&(PIPE_NOWAIT | PIPE_READMODE_BYTE)),
                None,
                None,
            )
            .context("Failed to enable nonblocking service pipe")?;
        }
        let request = format!("{req_json}\n");
        let mut pending = request.as_bytes();
        while !pending.is_empty() {
            anyhow::ensure!(Instant::now() < deadline, "Service IPC deadline exceeded");
            match file.write(pending) {
                Ok(0) => wait_for_pipe_progress(deadline)?,
                Ok(n) => pending = &pending[n..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    wait_for_pipe_progress(deadline)?
                }
                Err(error) => return Err(error).context("Failed to write to service pipe"),
            }
        }
        // File::flush is unnecessary; FlushFileBuffers on a pipe could wait for a reader.
        let mut buf = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            anyhow::ensure!(Instant::now() < deadline, "Service IPC deadline exceeded");
            // In PIPE_NOWAIT byte mode an empty open pipe can return zero bytes;
            // disconnection errors are handled below rather than treating this as EOF.
            match file.read(&mut byte) {
                Ok(0) => {
                    wait_for_pipe_progress(deadline)?;
                    continue;
                }
                Ok(_) => {}
                Err(error)
                    if error.raw_os_error() == Some(ERROR_NO_DATA.0 as i32)
                        || error.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    wait_for_pipe_progress(deadline)?;
                    continue;
                }
                Err(error) => return Err(error).context("Failed to read from service pipe"),
            }
            if byte[0] == b'\n' {
                break;
            }
            anyhow::ensure!(buf.len() < 4096, "Service response exceeds 4096 bytes");
            buf.push(byte[0]);
        }
        let resp: ServiceResponse =
            serde_json::from_slice(&buf).context("Failed to parse service JSON response")?;
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    // Unique ephemeral pipe; does not contact or install the machine service.
    fn test_pipe(respond: bool) -> (String, mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let name = format!(r"\\.\pipe\PrismClientTest-{}", uuid::Uuid::new_v4());
        let server_name = name.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut pipe = tokio::net::windows::named_pipe::ServerOptions::new()
                    .first_pipe_instance(true)
                    .create(&server_name)
                    .unwrap();
                ready_tx.send(()).unwrap();
                tokio::time::timeout(Duration::from_secs(3), pipe.connect())
                    .await
                    .unwrap()
                    .unwrap();
                if respond {
                    loop {
                        let mut byte = [0u8; 1];
                        pipe.read_exact(&mut byte).await.unwrap();
                        if byte[0] == b'\n' {
                            break;
                        }
                    }
                    pipe.write_all(br#"{"status":"ok","message":"po"#)
                        .await
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    pipe.write_all(b"ng\"}\n").await.unwrap();
                }
                // Keep pipe open until client returns, including timeout cases.
                let _ = release_rx.recv_timeout(Duration::from_secs(3));
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        (name, release_tx, worker)
    }

    #[test]
    fn held_open_pipe_times_out_without_waiting_for_server_close() {
        let (name, release, worker) = test_pipe(false);
        let started = Instant::now();
        let result =
            ServiceClient::exchange(&name, r#"{"command":"ping"}"#, Duration::from_millis(100));
        let elapsed = started.elapsed();
        let _ = release.send(());
        worker.join().unwrap();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("deadline exceeded"));
        assert!(elapsed >= Duration::from_millis(100));
        assert!(
            elapsed < Duration::from_secs(2),
            "client waited {elapsed:?}"
        );
    }

    #[test]
    fn fragmented_response_is_read_before_deadline() {
        let (name, release, worker) = test_pipe(true);
        let result =
            ServiceClient::exchange(&name, r#"{"command":"ping"}"#, Duration::from_secs(2));
        let _ = release.send(());
        worker.join().unwrap();
        let response = result.unwrap();
        assert_eq!(response.status, "ok");
        assert_eq!(response.message, "pong");
    }
}
