//! OpenReality Service — Privileged background Windows Service.
//!
//! Provides Prism service commands; it does not implement a SteamVR HMD driver.
//! Provides system-level management via a local Named Pipe (`\\.\pipe\PrismApertureService`):
//! - Realtime & High process priority boosts for VR streaming
//! - Process power throttling prevention (EcoQoS disabling)
//! - OpenXR active runtime registration in HKLM
//! - OVR Service (Oculus runtime) restart control
//! - Virtual audio endpoint and hardware acceleration optimization
//! - Service installation, uninstallation, and lifecycle management

use anyhow::{Context, Result};

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::{error, info, warn};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW,
    OpenServiceW, RegisterServiceCtrlHandlerW, SetServiceStatus, StartServiceCtrlDispatcherW,
    StartServiceW, SC_MANAGER_ALL_ACCESS, SERVICE_ALL_ACCESS, SERVICE_AUTO_START,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP,
    SERVICE_ERROR_NORMAL, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS,
    SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW,
    SERVICE_WIN32_OWN_PROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
    HIGH_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    PROCESS_SET_INFORMATION,
};

pub const SERVICE_NAME: &str = "PrismApertureService";
pub const SERVICE_DISPLAY_NAME: &str = "Prism Aperture Service";
pub const PIPE_NAME: &str = r"\\.\pipe\PrismApertureService";

static IS_RUNNING: AtomicBool = AtomicBool::new(false);
static mut SERVICE_STATUS_H: Option<SERVICE_STATUS_HANDLE> = None;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("--service");

    match command {
        "--install" => install_service(),
        "--uninstall" => uninstall_service(),
        "--start" => start_service_cli(),
        "--stop" => stop_service_cli(),
        "--run" | "-r" => run_interactive(),
        "--service" => run_as_service(),
        other => anyhow::bail!("Unknown service argument: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Service Control Manager Integration
// ---------------------------------------------------------------------------

fn run_as_service() -> Result<()> {
    info!("Starting OpenReality Service via Service Control Manager...");

    let service_name_hstring = HSTRING::from(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: windows::core::PWSTR(service_name_hstring.as_ptr() as *mut _),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: windows::core::PWSTR(ptr::null_mut()),
            lpServiceProc: None,
        },
    ];

    unsafe {
        let result = StartServiceCtrlDispatcherW(table.as_ptr());
        if result.is_err() {
            return Err(result.unwrap_err().into());
        }
    }

    Ok(())
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows::core::PWSTR) {
    let name = HSTRING::from(SERVICE_NAME);
    let status_handle =
        match RegisterServiceCtrlHandlerW(PCWSTR(name.as_ptr()), Some(service_ctrl_handler)) {
            Ok(h) => h,
            Err(_) => return,
        };

    SERVICE_STATUS_H = Some(status_handle);

    update_status(SERVICE_START_PENDING, 1000);
    IS_RUNNING.store(true, Ordering::SeqCst);
    update_status(SERVICE_RUNNING, 0);

    info!("OpenReality Service is now RUNNING");

    // Run the IPC named pipe server on a background thread
    let _ = std::thread::spawn(|| {
        run_tokio_server();
    });

    while IS_RUNNING.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    update_status(SERVICE_STOPPED, 0);
    info!("OpenReality Service has STOPPED");
}

unsafe extern "system" fn service_ctrl_handler(control: u32) {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            update_status(SERVICE_STOP_PENDING, 2000);
            IS_RUNNING.store(false, Ordering::SeqCst);
        }
        SERVICE_CONTROL_INTERROGATE => {
            update_status(SERVICE_RUNNING, 0);
        }
        _ => {}
    }
}

fn update_status(
    current_state: windows::Win32::System::Services::SERVICE_STATUS_CURRENT_STATE,
    wait_hint_ms: u32,
) {
    unsafe {
        if let Some(handle) = SERVICE_STATUS_H {
            let mut status = SERVICE_STATUS {
                dwServiceType: SERVICE_WIN32_OWN_PROCESS,
                dwCurrentState: current_state,
                dwControlsAccepted: if current_state == SERVICE_RUNNING {
                    windows::Win32::System::Services::SERVICE_ACCEPT_STOP
                        | windows::Win32::System::Services::SERVICE_ACCEPT_SHUTDOWN
                } else {
                    windows::Win32::System::Services::SERVICE_ACCEPT_STOP
                },
                dwWin32ExitCode: 0,
                dwServiceSpecificExitCode: 0,
                dwCheckPoint: 0,
                dwWaitHint: wait_hint_ms,
            };
            let _ = SetServiceStatus(handle, &mut status);
        }
    }
}

fn run_interactive() -> Result<()> {
    info!("Starting OpenReality Service in interactive console mode...");
    IS_RUNNING.store(true, Ordering::SeqCst);
    run_tokio_server();
    Ok(())
}

// ---------------------------------------------------------------------------
// Named Pipe IPC Server via Tokio
// ---------------------------------------------------------------------------

fn run_tokio_server() {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to create tokio runtime: {e}");
            return;
        }
    };

    rt.block_on(async {
        info!("Listening on named pipe: {PIPE_NAME}");

        while IS_RUNNING.load(Ordering::SeqCst) {
            let server = match tokio::net::windows::named_pipe::ServerOptions::new()
                .reject_remote_clients(true)
                .create(PIPE_NAME)
            {
                Ok(s) => s,
                Err(e) => {
                    warn!("Could not create service pipe: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
            };

            if matches!(
                tokio::time::timeout(std::time::Duration::from_millis(500), server.connect()).await,
                Ok(Ok(()))
            ) {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut server = server;
                    // Newline framing, bounded memory and deadlines for partial clients.
                    let transaction = async {
                        let mut request = Vec::new();
                        loop {
                            let mut byte = [0u8; 1];
                            if server.read(&mut byte).await? == 0 {
                                return Ok::<(), std::io::Error>(());
                            }
                            if byte[0] == b'\n' {
                                break;
                            }
                            if request.len() >= 4096 {
                                return Ok(());
                            }
                            request.push(byte[0]);
                        }
                        let response = process_command(&String::from_utf8_lossy(&request));
                        server.write_all(response.as_bytes()).await?;
                        server.write_all(b"\n").await?;
                        Ok(())
                    };
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(5), transaction).await;
                });
            }
        }
    });
}

#[derive(serde::Deserialize)]
struct ServiceRequest {
    command: String,
    pid: Option<u32>,
    path: Option<String>,
}

#[derive(serde::Serialize)]
struct ServiceResponse {
    status: String,
    message: String,
}

fn process_command(json_str: &str) -> String {
    let req: Result<ServiceRequest, _> = serde_json::from_str(json_str);
    let req = match req {
        Ok(r) => r,
        Err(e) => {
            return serde_json::to_string(&ServiceResponse {
                status: "error".to_string(),
                message: format!("Invalid JSON: {e}"),
            })
            .unwrap_or_default()
        }
    };

    let result = match req.command.as_str() {
        "ping" => Ok("pong".to_string()),
        "use_realtime_priority" => {
            if let Some(pid) = req.pid {
                set_realtime_priority(pid)
            } else {
                Err(anyhow::anyhow!("Missing pid argument"))
            }
        }
        "disable_process_throttling" => {
            if let Some(pid) = req.pid {
                disable_process_throttling(pid)
            } else {
                Err(anyhow::anyhow!("Missing pid argument"))
            }
        }
        "set_openxr_runtime" => {
            if let Some(path) = &req.path {
                set_openxr_runtime(path)
            } else {
                Err(anyhow::anyhow!("Missing path argument"))
            }
        }
        "restart_ovr_service" => restart_ovr_service(),
        "disable_hags" => disable_hags(),
        other => Err(anyhow::anyhow!("Unknown command: {other}")),
    };

    match result {
        Ok(msg) => serde_json::to_string(&ServiceResponse {
            status: "ok".to_string(),
            message: msg,
        })
        .unwrap_or_default(),
        Err(e) => serde_json::to_string(&ServiceResponse {
            status: "error".to_string(),
            message: e.to_string(),
        })
        .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Service Handlers (Ported from VirtualDesktop.Service)
// ---------------------------------------------------------------------------

fn set_realtime_priority(pid: u32) -> Result<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_SET_INFORMATION, false, pid)
            .context("Failed to open process for priority change")?;

        let res = SetPriorityClass(handle, HIGH_PRIORITY_CLASS);
        let _ = CloseHandle(handle);

        if res.is_ok() {
            info!("Successfully elevated PID {pid} to HIGH_PRIORITY_CLASS");
            Ok(format!("PID {pid} priority elevated"))
        } else {
            Err(anyhow::anyhow!("Failed to set process priority"))
        }
    }
}

fn disable_process_throttling(pid: u32) -> Result<String> {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_SET_INFORMATION, false, pid)
            .context("Failed to open process for power throttling control")?;
        let result = SetProcessInformation(
            handle,
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            std::mem::size_of_val(&state) as u32,
        );
        let _ = CloseHandle(handle);
        result.context("Failed to disable process execution-speed throttling")?;
    }
    Ok(format!("Power throttling disabled for PID {pid}"))
}

fn set_openxr_runtime(manifest_path: &str) -> Result<String> {
    unsafe {
        let key_path = HSTRING::from(r"SOFTWARE\Khronos\OpenXR\1");
        let val_name = HSTRING::from("ActiveRuntime");
        let mut hkey = HKEY::default();

        let res = RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key_path.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut hkey,
            None,
        );

        if res.is_err() {
            return Err(anyhow::anyhow!("Failed to open OpenXR registry key"));
        }

        let val_data: Vec<u16> = manifest_path
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let set_res = RegSetValueExW(
            hkey,
            PCWSTR(val_name.as_ptr()),
            0,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                val_data.as_ptr() as *const u8,
                val_data.len() * 2,
            )),
        );

        let _ = RegCloseKey(hkey);

        if set_res.is_ok() {
            info!("Registered active OpenXR runtime: {manifest_path}");
            Ok(format!("OpenXR runtime set to {manifest_path}"))
        } else {
            Err(anyhow::anyhow!(
                "Failed to write OpenXR ActiveRuntime value"
            ))
        }
    }
}

fn restart_ovr_service() -> Result<String> {
    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)
            .context("Failed to open Service Control Manager")?;

        let ovr_name = HSTRING::from("OVRService");
        let service = OpenServiceW(scm, PCWSTR(ovr_name.as_ptr()), SERVICE_ALL_ACCESS);

        if let Ok(svc) = service {
            let mut status = SERVICE_STATUS::default();
            let _ = ControlService(svc, SERVICE_CONTROL_STOP, &mut status);
            std::thread::sleep(std::time::Duration::from_secs(1));
            let _ = StartServiceW(svc, None);
            let _ = CloseServiceHandle(svc);
            let _ = CloseServiceHandle(scm);
            info!("Restarted OVRService");
            Ok("OVRService restarted".to_string())
        } else {
            let _ = CloseServiceHandle(scm);
            Ok("OVRService not found or not running".to_string())
        }
    }
}

fn disable_hags() -> Result<String> {
    unsafe {
        let key_path = HSTRING::from(r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers");
        let val_name = HSTRING::from("HwSchMode");
        let mut hkey = HKEY::default();

        if RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key_path.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut hkey,
            None,
        )
        .is_ok()
        {
            let val: u32 = 1; // 1 = disabled
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(val_name.as_ptr()),
                0,
                windows::Win32::System::Registry::REG_DWORD,
                Some(std::slice::from_raw_parts(
                    &val as *const u32 as *const u8,
                    4,
                )),
            );
            let _ = RegCloseKey(hkey);
            info!("HAGS disabled in registry");
            Ok("HAGS disabled in registry".to_string())
        } else {
            Err(anyhow::anyhow!("Failed to open GraphicsDrivers key"))
        }
    }
}

// ---------------------------------------------------------------------------
// Installation & Uninstallation
// ---------------------------------------------------------------------------

fn install_service() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let exe_str = format!("\"{}\" --service", exe_path.display());
    info!("Installing OpenReality Service from {exe_str}...");

    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)
            .context("Failed to open Service Control Manager (run as Administrator)")?;

        let svc_name = HSTRING::from(SERVICE_NAME);
        let disp_name = HSTRING::from(SERVICE_DISPLAY_NAME);
        let bin_path = HSTRING::from(exe_str);

        let svc = CreateServiceW(
            scm,
            PCWSTR(svc_name.as_ptr()),
            PCWSTR(disp_name.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(bin_path.as_ptr()),
            None,
            None,
            None,
            None,
            None,
        );

        let _ = CloseServiceHandle(scm);

        if let Ok(s) = svc {
            let _ = StartServiceW(s, None);
            let _ = CloseServiceHandle(s);
            println!("OpenReality Service installed and started successfully!");
            Ok(())
        } else {
            let err = GetLastError();
            if err.0 == 1073 {
                // ERROR_SERVICE_EXISTS
                println!("OpenReality Service already installed.");
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "Failed to create service: error code {}",
                    err.0
                ))
            }
        }
    }
}

fn uninstall_service() -> Result<()> {
    info!("Uninstalling OpenReality Service...");
    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)
            .context("Failed to open Service Control Manager")?;

        let svc_name = HSTRING::from(SERVICE_NAME);
        let svc = OpenServiceW(scm, PCWSTR(svc_name.as_ptr()), SERVICE_ALL_ACCESS);

        if let Ok(s) = svc {
            let mut status = SERVICE_STATUS::default();
            let _ = ControlService(s, SERVICE_CONTROL_STOP, &mut status);
            let res = DeleteService(s);
            let _ = CloseServiceHandle(s);
            let _ = CloseServiceHandle(scm);

            if res.is_ok() {
                println!("OpenReality Service uninstalled successfully.");
                Ok(())
            } else {
                Err(anyhow::anyhow!("Failed to delete service"))
            }
        } else {
            let _ = CloseServiceHandle(scm);
            println!("OpenReality Service was not installed.");
            Ok(())
        }
    }
}

fn start_service_cli() -> Result<()> {
    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
        let svc_name = HSTRING::from(SERVICE_NAME);
        let svc = OpenServiceW(scm, PCWSTR(svc_name.as_ptr()), SERVICE_ALL_ACCESS)?;
        let _ = StartServiceW(svc, None);
        let _ = CloseServiceHandle(svc);
        let _ = CloseServiceHandle(scm);
        println!("Service start requested.");
        Ok(())
    }
}

fn stop_service_cli() -> Result<()> {
    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS)?;
        let svc_name = HSTRING::from(SERVICE_NAME);
        let svc = OpenServiceW(scm, PCWSTR(svc_name.as_ptr()), SERVICE_ALL_ACCESS)?;
        let mut status = SERVICE_STATUS::default();
        let _ = ControlService(svc, SERVICE_CONTROL_STOP, &mut status);
        let _ = CloseServiceHandle(svc);
        let _ = CloseServiceHandle(scm);
        println!("Service stop requested.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttling_failure_is_not_reported_as_success() {
        let response: serde_json::Value = serde_json::from_str(&process_command(
            r#"{"command":"disable_process_throttling","pid":0}"#,
        ))
        .unwrap();
        assert_eq!(response["status"], "error");
        assert!(response["message"]
            .as_str()
            .unwrap()
            .contains("Failed to open process"));
    }

    #[test]
    fn ping_and_invalid_requests() {
        let ping: serde_json::Value =
            serde_json::from_str(&process_command(r#"{"command":"ping"}"#)).unwrap();
        assert_eq!(ping["message"], "pong");
        for request in [
            "{",
            r#"{"command":"unknown"}"#,
            r#"{"command":"use_realtime_priority"}"#,
        ] {
            let response: serde_json::Value =
                serde_json::from_str(&process_command(request)).unwrap();
            assert_eq!(response["status"], "error");
        }
    }
}
