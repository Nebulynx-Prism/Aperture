//! Background stable-release checks and a separate validated updater handoff.
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use std::{
    ffi::c_void,
    ptr,
    sync::mpsc,
    time::{Duration, Instant},
};
use windows::{
    core::{w, PCWSTR},
    Win32::Networking::WinHttp::*,
};

const RELEASE_PREFIX: &str = "https://github.com/Nebulynx-Prism/Aperture/releases/";
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

struct Internet(*mut c_void);
impl Internet {
    fn checked(handle: *mut c_void) -> Result<Self> {
        ensure!(
            !handle.is_null(),
            "Windows HTTP session could not be created"
        );
        Ok(Self(handle))
    }
}
impl Drop for Internet {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub page_url: String,
    pub installer_url: Option<String>,
    pub package_url: Option<String>,
    pub urgent: bool,
}
#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}
#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

fn newer_release(bytes: &[u8], installed: &str) -> Result<Option<Release>> {
    ensure!(bytes.len() <= 512 * 1024, "Release response is too large");
    let api: ApiRelease =
        serde_json::from_slice(bytes).context("Invalid GitHub release response")?;
    ensure!(
        api.tag_name.len() <= 128 && api.html_url.starts_with(RELEASE_PREFIX),
        "Unexpected release metadata"
    );
    if api.draft || api.prerelease {
        return Ok(None);
    }
    let version = semver::Version::parse(api.tag_name.trim_start_matches(['v', 'V']))
        .context("Release tags must use semantic versions, for example v0.1.1")?;
    let current = semver::Version::parse(installed)?;
    if !version.pre.is_empty() || version.cmp_precedence(&current) != std::cmp::Ordering::Greater {
        return Ok(None);
    }
    let urgent = api.assets.iter().any(|a| {
        a.name == "Aperture-UrgentUpdate.zip"
            && a.browser_download_url
                .starts_with(&format!("{RELEASE_PREFIX}download/"))
    });
    let package_name = if urgent {
        "Aperture-UrgentUpdate.zip"
    } else {
        "Aperture-UpdatePackage.zip"
    };
    let package_url = api
        .assets
        .iter()
        .find(|a| {
            a.name == package_name
                && a.browser_download_url
                    .starts_with(&format!("{RELEASE_PREFIX}download/"))
        })
        .map(|a| a.browser_download_url.clone());
    let installer_url = api
        .assets
        .into_iter()
        .find(|a| {
            ["PrismAperture-Setup.exe", "Aperture-Setup.exe"]
                .iter()
                .any(|n| a.name.eq_ignore_ascii_case(n))
                && a.browser_download_url
                    .starts_with(&format!("{RELEASE_PREFIX}download/"))
        })
        .map(|a| a.browser_download_url);
    Ok(Some(Release {
        version: version.to_string(),
        page_url: api.html_url,
        installer_url,
        package_url,
        urgent,
    }))
}

fn check_latest() -> Result<Option<Release>> {
    unsafe {
        let session = Internet::checked(WinHttpOpen(
            w!("Aperture-Server-Update-Check/0.1"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 5000, 5000, 5000, 5000)?;
        let connection =
            Internet::checked(WinHttpConnect(session.0, w!("api.github.com"), 443, 0))?;
        let request = Internet::checked(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            w!("/repos/Nebulynx-Prism/Aperture/releases/latest"),
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        let headers: Vec<u16> =
            "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n"
                .encode_utf16()
                .collect();
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0)?;
        WinHttpReceiveResponse(request.0, ptr::null_mut())?;
        let mut status = 0u32;
        let mut length = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut length,
            ptr::null_mut(),
        )?;
        ensure!(
            status != 404,
            "No public stable release found; the repository may be private or have no releases yet"
        );
        ensure!(
            status != 403 && status != 429,
            "GitHub rate limit reached; try again later"
        );
        ensure!(status == 200, "GitHub returned HTTP {status}");
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut response = Vec::new();
        loop {
            ensure!(
                Instant::now() < deadline,
                "Release response deadline exceeded"
            );
            let mut chunk = [0u8; 8192];
            let mut received = 0;
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut received,
            )?;
            if received == 0 {
                break;
            }
            ensure!(
                response.len() + received as usize <= 512 * 1024,
                "Release response is too large"
            );
            response.extend_from_slice(&chunk[..received as usize]);
        }
        newer_release(&response, env!("CARGO_PKG_VERSION"))
    }
}

pub struct UpdateChecker {
    receiver: Option<mpsc::Receiver<Result<Option<Release>>>>,
    last_check: Option<Instant>,
    pub message: String,
    pub release: Option<Release>,
}
impl Default for UpdateChecker {
    fn default() -> Self {
        Self {
            receiver: None,
            last_check: None,
            message: "Not checked yet".into(),
            release: None,
        }
    }
}
impl UpdateChecker {
    pub fn checking(&self) -> bool {
        self.receiver.is_some()
    }
    pub fn due(&self) -> bool {
        !self.checking()
            && self
                .last_check
                .map_or(true, |t| t.elapsed() >= CHECK_INTERVAL)
    }
    pub fn start(&mut self) {
        if self.checking() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.last_check = Some(Instant::now());
        self.release = None;
        self.message = "Checking GitHub releases...".into();
        std::thread::spawn(move || {
            let _ = sender.send(check_latest());
        });
    }
    pub fn poll(&mut self) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.receiver = None;
                self.release = None;
                match result {
                    Ok(Some(release)) => {
                        self.message = format!("Aperture {} is available", release.version);
                        self.release = Some(release);
                    }
                    Ok(None) => self.message = "You have the latest stable version".into(),
                    Err(error) => self.message = format!("Update check failed: {error:#}"),
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                self.message = "Update check stopped; try again".into();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn api(tag: &str, prerelease: bool, url: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"tag_name":tag,"html_url":url,"prerelease":prerelease,
            "assets":[{"name":"PrismAperture-Setup.exe","browser_download_url":format!("{RELEASE_PREFIX}download/v1.0.0/PrismAperture-Setup.exe")}]})).unwrap()
    }
    #[test]
    fn stable_version_order_and_prereleases() {
        let url = format!("{RELEASE_PREFIX}tag/v0.10.0");
        assert!(newer_release(&api("v0.10.0", false, &url), "0.9.0")
            .unwrap()
            .is_some());
        assert!(newer_release(&api("v0.9.0", false, &url), "0.10.0")
            .unwrap()
            .is_none());
        assert!(newer_release(&api("v0.10.0", true, &url), "0.9.0")
            .unwrap()
            .is_none());
        assert!(newer_release(&api("v0.10.0-beta.1", false, &url), "0.9.0")
            .unwrap()
            .is_none());
        assert!(newer_release(&api("v0.9.0+other", false, &url), "0.9.0")
            .unwrap()
            .is_none());
    }
    #[test]
    fn release_links_and_metadata_are_validated() {
        assert!(newer_release(&api("v1.0.0", false, "https://example.com/"), "0.1.0").is_err());
        assert!(newer_release(b"invalid", "0.1.0").is_err());
        let release = newer_release(
            &api("v1.0.0", false, &format!("{RELEASE_PREFIX}tag/v1.0.0")),
            "0.1.0",
        )
        .unwrap()
        .unwrap();
        assert!(release.installer_url.unwrap().starts_with(RELEASE_PREFIX));
    }
}

#[cfg(test)]
mod package_tests {
    use super::*;
    fn response(names:&[&str])->Vec<u8>{
        serde_json::to_vec(&serde_json::json!({"tag_name":"v9.9.9","html_url":format!("{RELEASE_PREFIX}tag/v9.9.9"),"assets":names.iter().map(|name|serde_json::json!({"name":name,"browser_download_url":format!("{RELEASE_PREFIX}download/v9.9.9/{name}")})).collect::<Vec<_>>()})).unwrap()
    }
    #[test]
    fn normal_zip_and_urgent_precedence(){
        let normal=newer_release(&response(&["Aperture-UpdatePackage.zip"]),"0.1.0").unwrap().unwrap();
        assert!(!normal.urgent);assert!(normal.package_url.unwrap().ends_with("Aperture-UpdatePackage.zip"));
        let urgent=newer_release(&response(&["Aperture-UpdatePackage.zip","Aperture-UrgentUpdate.zip"]),"0.1.0").unwrap().unwrap();
        assert!(urgent.urgent);assert!(urgent.package_url.unwrap().ends_with("Aperture-UrgentUpdate.zip"));
    }
    #[test]
    fn external_zip_url_is_not_auto_applied(){
        let mut value:serde_json::Value=serde_json::from_slice(&response(&["Aperture-UrgentUpdate.zip"])).unwrap();
        value["assets"][0]["browser_download_url"]=serde_json::Value::String("https://example.com/Aperture-UrgentUpdate.zip".into());
        let release=newer_release(&serde_json::to_vec(&value).unwrap(),"0.1.0").unwrap().unwrap();
        assert!(!release.urgent);assert!(release.package_url.is_none());
    }
}

