//! Explicit output contract captured from the working Aperture stream, not an
//! invented HMD or a resolution reduction chosen to meet a performance target.
use std::{ffi::c_void, os::windows::ffi::OsStringExt, path::PathBuf};
#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleExW(flags: u32, address: *const u16, module: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(module: *mut c_void, path: *mut u16, capacity: u32) -> u32;
}
pub fn output_dimensions() -> Option<(u32, u32)> {
    let mut module = std::ptr::null_mut();
    let mut path = vec![0u16; 32768];
    unsafe {
        if GetModuleHandleExW(
            6,
            crate::aperture_start as *const () as *const u16,
            &mut module,
        ) == 0
        {
            return None;
        }
        let len = GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) as usize;
        if len == 0 || len >= path.len() {
            return None;
        }
        path.truncate(len);
    }
    let library = PathBuf::from(std::ffi::OsString::from_wide(&path));
    let root = library.parent()?.parent()?.parent()?;
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("resources/display_contract.json")).ok()?)
            .ok()?;
    let width = u32::try_from(value["stream_eye_width"].as_u64()?).ok()?;
    let height = u32::try_from(value["stream_eye_height"].as_u64()?).ok()?;
    if !(256..=2048).contains(&width) || !(256..=2048).contains(&height) {
        return None;
    }
    Some((width * 2, height))
}
