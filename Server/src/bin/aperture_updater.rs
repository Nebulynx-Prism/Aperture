//! Isolated updater worker; installer application happens only after package verification.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
use anyhow::{Context, Result};
use std::os::windows::process::CommandExt;
use windows::{
    core::HSTRING,
    Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
};
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| -> Result<String> {
        let i = args.iter().position(|s| s == name).context(format!(
            "Missing {name}; start updates from Aperture Server"
        ))?;
        args.get(i + 1).cloned().context("Missing updater argument")
    };
    let job = std::path::PathBuf::from(value("--job")?).canonicalize()?;
    let install = std::path::PathBuf::from(value("--install-dir")?).canonicalize()?;
    anyhow::ensure!(
        install.join("prism-aperture-server.exe").is_file(),
        "Aperture Server not found in target installation"
    );
    let pid = value("--parent-pid")?.parse::<u32>()?;
    anyhow::ensure!(pid > 0, "Invalid parent PID");
    let version = value("--version")?;
    semver::Version::parse(&version)?;
    let script = job.join("apply-update.ps1");
    std::fs::write(&script, include_str!("../../tools/apply_update.ps1"))?;
    let mut command = std::process::Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script)
        .arg("-JobDirectory")
        .arg(&job)
        .arg("-InstallDirectory")
        .arg(&install)
        .arg("-ParentPid")
        .arg(pid.to_string())
        .arg("-ExpectedVersion")
        .arg(&version)
        .creation_flags(0x08000000);
    if args.iter().any(|s| s == "--urgent") {
        command.arg("-Urgent");
    }
    let output = command.output().context("Could not start updater worker")?;
    if !output.status.success() {
        std::fs::write(job.join("worker-error.log"), &output.stderr)?;
        anyhow::bail!("Update failed; details are saved in {}", job.display());
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        unsafe {
            let _ = MessageBoxW(
                None,
                &HSTRING::from(format!("{error:#}")),
                &HSTRING::from("Aperture Updater"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}
