//! Optional diagnostic console. GUI binaries never create a console at launch.
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::{core::w, Win32::System::Console::*};
static OWN_CONSOLE: AtomicBool = AtomicBool::new(false);

pub fn set_visible(visible: bool) -> Result<()> {
    unsafe {
        if visible {
            if GetConsoleWindow().is_invalid() {
                AllocConsole().context("Could not open diagnostic console")?;
                OWN_CONSOLE.store(true, Ordering::Relaxed);
                let _ = SetConsoleTitleW(w!("Aperture Server Diagnostics"));
            }
        } else if OWN_CONSOLE.load(Ordering::Relaxed) {
            FreeConsole().context("Could not close diagnostic console")?;
            OWN_CONSOLE.store(false, Ordering::Relaxed);
        }
    }
    Ok(())
}

pub fn initialize(gui_console: bool, command_line: bool) -> Result<()> {
    if !command_line {
        return set_visible(gui_console);
    }
    unsafe {
        // Preserve redirected output when attaching to a caller's terminal.
        let output = GetStdHandle(STD_OUTPUT_HANDLE)
            .ok()
            .filter(|h| !h.is_invalid());
        let error = GetStdHandle(STD_ERROR_HANDLE)
            .ok()
            .filter(|h| !h.is_invalid());
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err()
            && output.is_none()
            && GetConsoleWindow().is_invalid()
        {
            AllocConsole().context("Could not open command-line console")?;
        }
        if let Some(handle) = output {
            SetStdHandle(STD_OUTPUT_HANDLE, handle)?;
        }
        if let Some(handle) = error {
            SetStdHandle(STD_ERROR_HANDLE, handle)?;
        }
    }
    Ok(())
}
