//! Updates from the latest GitHub release.
//!
//! The release's `ClaudeUsageWidget.exe` is downloaded next to the running exe
//! and checked against the SHA-256 GitHub publishes for it. Windows won't let a
//! running exe be overwritten but does let it be renamed, so the running exe
//! moves aside to `<name>.old`, the new one takes its name, and the new one is
//! started. It deletes the `.old` file once this process has exited.

use crate::http;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Security::Cryptography::{
    BCryptCloseAlgorithmProvider, BCryptHash, BCryptOpenAlgorithmProvider, BCRYPT_SHA256_ALGORITHM,
};

const REPO: &str = "AbdoSarmini/claude-usage-widget-for-windows-taskbar";
const ASSET: &str = "ClaudeUsageWidget.exe";
/// Passed to the new exe so it waits for this one to exit.
pub const UPDATED_ARG: &str = "--updated";

#[derive(Clone)]
pub struct Release {
    /// e.g. "v1.0.2".
    pub tag: String,
    /// The release's page on GitHub.
    pub page: String,
    download_path: String,
    sha256: [u8; 32],
}

pub enum InstallError {
    /// The exe's folder isn't writable (e.g. under Program Files).
    NoAccess,
    Other(String),
}

pub fn releases_page() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

/// Returns the latest release if it's newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    check_newer_than(env!("CARGO_PKG_VERSION"))
}

fn check_newer_than(current: &str) -> Result<Option<Release>, String> {
    let resp = http::request(
        "GET",
        "api.github.com",
        &format!("/repos/{REPO}/releases/latest"),
        "Accept: application/vnd.github+json",
        None,
    )?;
    if resp.status != 200 {
        return Err(format!("GitHub answered with HTTP {}", resp.status));
    }
    let v: Value =
        serde_json::from_slice(&resp.body).map_err(|_| "Bad response from GitHub".to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or_default().to_string();
    let (Some(latest), Some(current)) = (parse_version(&tag), parse_version(current))
    else {
        return Err(format!("Unrecognized release version \"{tag}\""));
    };
    if latest <= current {
        return Ok(None);
    }
    let asset = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(ASSET)))
        .ok_or_else(|| format!("Release {tag} has no {ASSET}"))?;
    let url = asset["browser_download_url"].as_str().unwrap_or_default();
    let download_path = url
        .strip_prefix("https://github.com")
        .filter(|p| p.starts_with('/'))
        .ok_or_else(|| format!("Unexpected download link for {tag}"))?
        .to_string();
    // Without a checksum the download can't be checked, so don't offer it.
    let sha256 = asset["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .and_then(parse_hex32)
        .ok_or_else(|| format!("Release {tag} has no checksum for {ASSET}"))?;
    let page = v["html_url"].as_str().map(str::to_string).unwrap_or_else(releases_page);
    Ok(Some(Release { tag, page, download_path, sha256 }))
}

/// Downloads the release and puts it in place of `exe`. On success the new exe
/// is at `exe` and the old one at `<exe>.old`; the caller starts the new one
/// and exits.
pub fn install(rel: &Release, exe: &Path) -> Result<(), InstallError> {
    let other = InstallError::Other;
    // Writing next to the exe first finds an unwritable folder before anything changes.
    let new = sibling(exe, "new");
    fs::write(&new, b"").map_err(io_err)?;
    let result = (|| {
        let resp = http::request("GET", "github.com", &rel.download_path, "Accept: application/octet-stream", None)
            .map_err(|e| other(format!("Download failed: {e}")))?;
        if resp.status != 200 {
            return Err(other(format!("Download failed (HTTP {})", resp.status)));
        }
        if sha256(&resp.body).as_ref() != Some(&rel.sha256) {
            return Err(other("The download didn't match the release's checksum.".into()));
        }
        if !resp.body.starts_with(b"MZ") {
            return Err(other("The download isn't a Windows program.".into()));
        }
        fs::write(&new, &resp.body).map_err(io_err)?;
        let old = sibling(exe, "old");
        let _ = fs::remove_file(&old);
        fs::rename(exe, &old).map_err(io_err)?;
        if let Err(e) = fs::rename(&new, exe) {
            let _ = fs::rename(&old, exe); // put the running exe back
            return Err(io_err(e));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&new);
    }
    result
}

/// Deletes the `.old` exe an update left behind. It can stay locked for a
/// moment while the previous process exits, so this retries for a while.
pub fn clean_up_old(exe: &Path) {
    let old = sibling(exe, "old");
    if !old.exists() {
        return;
    }
    for _ in 0..50 {
        if fs::remove_file(&old).is_ok() || !old.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// `C:\x\ClaudeUsageWidget.exe` -> `C:\x\ClaudeUsageWidget.exe.<ext>`.
fn sibling(exe: &Path, ext: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

fn io_err(e: std::io::Error) -> InstallError {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        InstallError::NoAccess
    } else {
        InstallError::Other(format!("Could not replace the widget: {e}"))
    }
}

/// "v1.2.3" or "1.2.3" -> (1, 2, 3).
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let v = (it.next()??, it.next()??, it.next().unwrap_or(Some(0))?);
    it.next().is_none().then_some(v)
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// SHA-256 with Windows' built-in CNG.
fn sha256(data: &[u8]) -> Option<[u8; 32]> {
    unsafe {
        let mut alg = std::ptr::null_mut();
        if BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_SHA256_ALGORITHM, std::ptr::null(), 0) != 0 {
            return None;
        }
        let mut out = [0u8; 32];
        let ok = BCryptHash(alg, std::ptr::null(), 0, data.as_ptr(), data.len() as u32, out.as_mut_ptr(), 32) == 0;
        BCryptCloseAlgorithmProvider(alg, 0);
        ok.then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.0.2"), Some((1, 0, 2)));
        assert_eq!(parse_version("1.10"), Some((1, 10, 0)));
        assert_eq!(parse_version("v1.0.2-beta"), None);
        assert!(parse_version("v1.0.10") > parse_version("v1.0.9"));
    }

    #[test]
    fn hash() {
        let want = parse_hex32("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap();
        assert_eq!(sha256(b"abc"), Some(want));
    }
}
