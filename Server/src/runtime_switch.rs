//! OpenXR activation is delegated to the installed privileged service, on user Apply.
use crate::settings::OpenXRRuntime;
use anyhow::{Context, Result};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
static APERTURE_SELECTED: AtomicBool = AtomicBool::new(false);
pub fn aperture_selected() -> bool {
    APERTURE_SELECTED.load(Ordering::Relaxed)
}
pub fn initialize() {
    // Read only: never change the system runtime merely by starting Aperture.
    if let Ok(output) = std::process::Command::new("reg.exe")
        .args([
            "query",
            r"HKLM\SOFTWARE\Khronos\OpenXR\1",
            "/v",
            "ActiveRuntime",
            "/reg:64",
        ])
        .creation_flags(0x08000000)
        .output()
    {
        let value = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
        APERTURE_SELECTED.store(
            value.contains("aperture-xr.json") || value.contains("prism-vdxr.json"),
            Ordering::Relaxed,
        );
    }
}
use std::os::windows::process::CommandExt;
fn manifest(runtime: OpenXRRuntime) -> Result<PathBuf> {
    if runtime == OpenXRRuntime::SteamVR {
        let path = crate::driver_bridge::steamvr_root()?.join("steamxr_win64.json");
        anyhow::ensure!(path.is_file(), "SteamVR OpenXR manifest not found");
        return Ok(path);
    }
    let adjacent = std::env::current_exe()?
        .parent()
        .context("No executable directory")?
        .join("openxr/aperture-xr.json");
    let development =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools/prism_vdxr/bin/aperture-xr.json");
    [adjacent, development]
        .into_iter()
        .find(|p| p.is_file())
        .context("ApertureXR is not installed. Select its OpenXR component in Aperture Setup.")
}
pub struct RuntimeSwitcher {
    pub choice: OpenXRRuntime,
    pub message: String,
    pending: Option<mpsc::Receiver<Result<OpenXRRuntime>>>,
}
impl RuntimeSwitcher {
    pub fn new(choice: OpenXRRuntime) -> Self {
        Self {
            choice,
            message: "Runtime is changed only when you click Apply.".into(),
            pending: None,
        }
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn apply(&mut self, restore: bool) {
        if self.busy() {
            return;
        }
        let choice = if restore {
            OpenXRRuntime::Automatic
        } else {
            self.choice
        };
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        self.message = "Applying OpenXR runtime...".into();
        std::thread::spawn(move || {
            let result = (|| -> Result<OpenXRRuntime> {
                if choice == OpenXRRuntime::Automatic {
                    crate::service_client::ServiceClient::restore_openxr_runtime()?;
                } else {
                    let path = manifest(choice)?.canonicalize()?;
                    crate::service_client::ServiceClient::set_openxr_runtime(
                        path.to_str().context("Runtime path is not Unicode")?,
                    )?;
                }
                initialize();
                Ok(choice)
            })();
            let _ = sender.send(result);
        });
    }
    pub fn poll(&mut self) -> Option<OpenXRRuntime> {
        let result = self.pending.as_ref()?.try_recv();
        match result {
            Ok(Ok(choice)) => {
                self.pending = None;
                self.choice = choice;
                self.message =
                    "OpenXR runtime applied. Restart your VR applications to use it.".into();
                Some(choice)
            }
            Ok(Err(error)) => {
                self.pending = None;
                self.message = format!("Runtime change failed: {error:#}");
                None
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.message = "Runtime service request stopped. Try again.".into();
                None
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}
