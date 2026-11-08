//! Starts the copied updater, then closes the server only when the verified package is ready.
use anyhow::{ensure, Context, Result};
use std::os::windows::process::CommandExt;
use std::{
    path::PathBuf,
    process::Child,
    time::{Duration, Instant},
};
#[derive(serde::Deserialize)]
struct Status {
    state: String,
    #[serde(default)]
    message: String,
}
pub enum Progress {
    Waiting,
    Ready,
    Failed(String),
}
pub struct UpdateJob {
    directory: PathBuf,
    child: Child,
    started: Instant,
}
impl UpdateJob {
    pub fn start(release: &crate::updater::Release) -> Result<Self> {
        ensure!(
            release.package_url.is_some(),
            "This release has no update ZIP"
        );
        let install = std::env::current_exe()?
            .parent()
            .context("Missing installation directory")?
            .to_path_buf();
        let updater = install.join("Aperture-Updater.exe");
        ensure!(
            updater.is_file(),
            "Aperture-Updater.exe is missing; reinstall Aperture"
        );
        let directory =
            std::env::temp_dir().join(format!("Aperture-Update-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory)?;
        let copied = directory.join("Aperture-Updater.exe");
        std::fs::copy(updater, &copied)?;
        let mut command = std::process::Command::new(copied);
        command
            .arg("--job")
            .arg(&directory)
            .arg("--install-dir")
            .arg(&install)
            .arg("--parent-pid")
            .arg(std::process::id().to_string())
            .arg("--version")
            .arg(&release.version)
            .creation_flags(0x08000000);
        if release.urgent {
            command.arg("--urgent");
        }
        let child = command.spawn()?;
        Ok(Self {
            directory,
            child,
            started: Instant::now(),
        })
    }
    pub fn poll(&mut self) -> Progress {
        let status_file = self.directory.join("status.json");
        if let Ok(metadata) = std::fs::metadata(&status_file) {
            if metadata.len() <= 65536 {
                if let Ok(bytes) = std::fs::read(&status_file) {
                    if let Ok(status) = serde_json::from_slice::<Status>(&bytes) {
                        if status.state == "ready" {
                            return Progress::Ready;
                        }
                        if status.state == "failed" {
                            return Progress::Failed(status.message);
                        }
                    }
                }
            }
        }
        if let Ok(Some(_)) = self.child.try_wait() {
            return Progress::Failed(format!(
                "Updater stopped. Check {}",
                self.directory.display()
            ));
        }
        if self.started.elapsed() > Duration::from_secs(360) {
            return Progress::Failed("Update preparation timed out; Aperture remains open.".into());
        }
        Progress::Waiting
    }
}
