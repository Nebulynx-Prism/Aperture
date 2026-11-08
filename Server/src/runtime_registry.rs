//! Privileged runtime registry operations. Called only for explicit service commands.
use anyhow::{ensure, Context, Result};
use std::path::Path;
use windows::{
    core::{w, PCWSTR},
    Win32::System::Registry::*,
};
const KEY: PCWSTR = w!("SOFTWARE\\Khronos\\OpenXR\\1");
const ACTIVE: PCWSTR = w!("ActiveRuntime");
const BACKUP: PCWSTR = w!("AperturePreviousRuntime");
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}
fn open() -> Result<Key> {
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            KEY,
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_WOW64_64KEY,
            None,
            &mut key,
            None,
        )
        .ok()
        .context("Could not open 64-bit OpenXR registry key")?;
        Ok(Key(key))
    }
}
fn read(key: HKEY, name: PCWSTR) -> Result<Option<String>> {
    unsafe {
        let mut bytes = 0;
        let result = RegGetValueW(
            key,
            PCWSTR::null(),
            name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        );
        if result.0 == 2 {
            return Ok(None);
        }
        result
            .ok()
            .context("Could not read OpenXR registry value")?;
        ensure!(bytes <= 65536, "OpenXR registry value exceeds limit");
        let mut text = vec![0u16; (bytes as usize + 1) / 2];
        RegGetValueW(
            key,
            PCWSTR::null(),
            name,
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
        .ok()?;
        let length = text.iter().position(|c| *c == 0).unwrap_or(text.len());
        Ok(Some(String::from_utf16(&text[..length])?))
    }
}
fn write(key: HKEY, name: PCWSTR, value: &str) -> Result<()> {
    unsafe {
        let text: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        RegSetValueExW(
            key,
            name,
            0,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                text.as_ptr().cast(),
                text.len() * 2,
            )),
        )
        .ok()?;
        Ok(())
    }
}
fn validate_manifest(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.is_file(),
        "Runtime manifest must be an existing absolute file"
    );
    ensure!(
        std::fs::metadata(path)?.len() <= 65536,
        "Runtime manifest exceeds size limit"
    );
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        manifest["file_format_version"].as_str() == Some("1.0.0"),
        "Unsupported OpenXR manifest version"
    );
    let library = manifest["runtime"]["library_path"]
        .as_str()
        .filter(|p| !p.is_empty())
        .context("Runtime manifest has no library_path")?;
    ensure!(
        path.parent()
            .context("Missing runtime directory")?
            .join(library)
            .is_file(),
        "Runtime library is missing"
    );
    Ok(())
}
pub fn activate(path: &str) -> Result<String> {
    validate_manifest(Path::new(path))?;
    let key = open()?;
    if read(key.0, BACKUP)?.is_none() {
        write(key.0, BACKUP, read(key.0, ACTIVE)?.as_deref().unwrap_or(""))?;
    }
    write(key.0, ACTIVE, path)?;
    Ok("OpenXR runtime applied; previous runtime preserved".into())
}
pub fn restore() -> Result<String> {
    let key = open()?;
    let previous =
        read(key.0, BACKUP)?.context("No previous OpenXR runtime was saved by Aperture")?;
    if previous.is_empty() {
        unsafe {
            let result = RegDeleteValueW(key.0, ACTIVE);
            if result.0 != 2 {
                result.ok()?;
            }
        }
    } else {
        write(key.0, ACTIVE, &previous)?;
    }
    unsafe {
        RegDeleteValueW(key.0, BACKUP).ok()?;
    }
    Ok("Previous OpenXR runtime restored".into())
}
pub fn restore_owned() -> Result<()> {
    let key = open()?;
    if let Some(active) = read(key.0, ACTIVE)? {
        let owned = std::env::current_exe()?
            .parent()
            .context("Missing installation directory")?
            .join("openxr/aperture-xr.json");
        let matches = Path::new(&active)
            .canonicalize()
            .ok()
            .zip(owned.canonicalize().ok())
            .map_or(false, |(a, b)| a == b);
        if matches && read(key.0, BACKUP)?.is_some() {
            restore()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_relative_or_invalid_runtime_is_rejected_before_registry_access() {
        assert!(validate_manifest(Path::new("relative.json")).is_err());
        assert!(activate("relative.json").is_err());
    }
    #[test]
    fn manifest_validation_requires_real_library() {
        let dir =
            std::env::temp_dir().join(format!("aperture-runtime-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let manifest = dir.join("runtime.json");
        std::fs::write(
            &manifest,
            br#"{"file_format_version":"1.0.0","runtime":{"library_path":"runtime.dll"}}"#,
        )
        .unwrap();
        assert!(validate_manifest(&manifest).is_err());
        std::fs::write(dir.join("runtime.dll"), b"test metadata only, never loaded").unwrap();
        assert!(validate_manifest(&manifest).is_ok());
        std::fs::remove_file(manifest).unwrap();
        std::fs::remove_file(dir.join("runtime.dll")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
