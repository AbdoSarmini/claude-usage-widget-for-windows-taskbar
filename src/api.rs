//! Reads Claude Code's OAuth login, refreshes it when needed, and fetches usage.
//!
//! Token refresh follows Claude Code's own protocol so both apps can share the
//! credentials file: take the same lock directories, re-read the file under the
//! lock, refresh, then atomically write the file back.

use crate::http;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const TOKEN_HOST: &str = "platform.claude.com";
const TOKEN_PATH: &str = "/v1/oauth/token";
const API_HOST: &str = "api.anthropic.com";
const USAGE_PATH: &str = "/api/oauth/usage";
/// Refresh when the token has less than this left (same margin as Claude Code).
const REFRESH_MARGIN_MS: i64 = 5 * 60 * 1000;
/// Claude Code treats a lock older than this as abandoned.
const LOCK_STALE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Default)]
pub struct Limit {
    pub percent: f64,
    /// Unix seconds.
    pub resets_at: Option<i64>,
}

#[derive(Clone, Copy, Default)]
pub struct Usage {
    pub five_hour: Option<Limit>,
    pub weekly: Option<Limit>,
}

pub enum FetchError {
    /// No Claude Code login found (not signed in).
    NoLogin,
    /// Login rejected and could not be refreshed: user must sign in again.
    LoginExpired,
    RateLimited,
    /// Another process is refreshing; try again shortly.
    Busy,
    Other(String),
}

impl FetchError {
    pub fn message(&self) -> String {
        match self {
            FetchError::NoLogin => "No Claude Code login found. Run `claude` and sign in.".into(),
            FetchError::LoginExpired => "Login expired. Run `claude` and sign in again.".into(),
            FetchError::RateLimited => "Rate limited, retrying later.".into(),
            FetchError::Busy => "Login is being refreshed, retrying.".into(),
            FetchError::Other(s) => s.clone(),
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

fn claude_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".claude")
}

fn creds_path() -> PathBuf {
    claude_dir().join(".credentials.json")
}

struct Creds {
    root: Value,
    access: String,
    refresh: Option<String>,
    expires_at: Option<i64>,
}

fn load_creds() -> Result<Creds, FetchError> {
    let text = fs::read_to_string(creds_path()).map_err(|_| FetchError::NoLogin)?;
    let root: Value = serde_json::from_str(&text).map_err(|_| FetchError::NoLogin)?;
    let o = &root["claudeAiOauth"];
    let access = o["accessToken"].as_str().ok_or(FetchError::NoLogin)?.to_string();
    Ok(Creds {
        refresh: o["refreshToken"].as_str().map(str::to_string),
        expires_at: o["expiresAt"].as_i64(),
        access,
        root,
    })
}

fn needs_refresh(c: &Creds) -> bool {
    matches!(c.expires_at, Some(e) if now_ms() + REFRESH_MARGIN_MS >= e)
}

/// A lock directory, same scheme as the `proper-lockfile` package Claude Code uses.
struct DirLock(PathBuf);
impl DirLock {
    fn acquire(path: PathBuf) -> Option<DirLock> {
        for _ in 0..2 {
            match fs::create_dir(&path) {
                Ok(()) => return Some(DirLock(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .map(|t| t.elapsed().unwrap_or_default() > LOCK_STALE)
                        .unwrap_or(false);
                    if !stale {
                        return None;
                    }
                    let _ = fs::remove_dir(&path);
                }
                Err(_) => return None,
            }
        }
        None
    }
}
impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

/// Refreshes the login. `rejected` is the token the server just refused
/// (forces a refresh even if it doesn't look expired yet).
fn refresh(rejected: Option<&str>) -> Result<String, FetchError> {
    let dir = claude_dir();
    let _lock = DirLock::acquire(dir.join(".oauth_refresh.lock")).ok_or(FetchError::Busy)?;
    let legacy = PathBuf::from(format!("{}.lock", dir.display()));
    let _legacy_lock = DirLock::acquire(legacy).ok_or(FetchError::Busy)?;

    // Re-read under the lock: someone else may have refreshed already.
    let mut c = load_creds()?;
    match rejected {
        Some(bad) if c.access != bad => return Ok(c.access),
        None if !needs_refresh(&c) => return Ok(c.access),
        _ => {}
    }
    let refresh_token = c.refresh.clone().ok_or(FetchError::LoginExpired)?;
    let scopes: Vec<String> = c.root["claudeAiOauth"]["scopes"]
        .as_array()
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let mut body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
    });
    if !scopes.is_empty() {
        body["scope"] = Value::String(scopes.join(" "));
    }
    let resp = http::request(
        "POST",
        TOKEN_HOST,
        TOKEN_PATH,
        "Content-Type: application/json\r\nAccept: application/json",
        Some(body.to_string().as_bytes()),
    )
    .map_err(FetchError::Other)?;
    match resp.status {
        200 => {}
        400 | 401 | 403 => return Err(FetchError::LoginExpired),
        429 => return Err(FetchError::RateLimited),
        s => return Err(FetchError::Other(format!("Token refresh failed (HTTP {s})"))),
    }
    let r: Value = serde_json::from_slice(&resp.body)
        .map_err(|_| FetchError::Other("Bad token response".into()))?;
    let access = r["access_token"]
        .as_str()
        .ok_or_else(|| FetchError::Other("Bad token response".into()))?
        .to_string();
    let now = now_ms();
    let o = &mut c.root["claudeAiOauth"];
    o["accessToken"] = Value::String(access.clone());
    if let Some(rt) = r["refresh_token"].as_str() {
        o["refreshToken"] = Value::String(rt.into());
    }
    if let Some(sec) = r["expires_in"].as_i64() {
        o["expiresAt"] = json!(now + sec * 1000);
    }
    if let Some(scope) = r["scope"].as_str() {
        o["scopes"] = json!(scope.split(' ').filter(|s| !s.is_empty()).collect::<Vec<_>>());
    }
    if let Some(sec) = r["refresh_token_expires_in"].as_i64() {
        let new = now + sec * 1000;
        let keep = o["refreshTokenExpiresAt"].as_i64().map_or(new, |old| old.min(new));
        o["refreshTokenExpiresAt"] = json!(keep);
    }
    // Atomic replace so Claude Code never sees a half-written file.
    let path = creds_path();
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp, serde_json::to_vec(&c.root).unwrap())
        .and_then(|_| fs::rename(&tmp, &path))
        .map_err(|e| {
            let _ = fs::remove_file(&tmp);
            FetchError::Other(format!("Could not save refreshed login: {e}"))
        })?;
    Ok(access)
}

fn get_usage(token: &str) -> Result<http::Response, FetchError> {
    http::request(
        "GET",
        API_HOST,
        USAGE_PATH,
        &format!(
            "Authorization: Bearer {token}\r\nanthropic-beta: oauth-2025-04-20\r\nAccept: application/json"
        ),
        None,
    )
    .map_err(FetchError::Other)
}

pub fn fetch() -> Result<Usage, FetchError> {
    let c = load_creds()?;
    let mut token = if needs_refresh(&c) { refresh(None)? } else { c.access };
    let mut resp = get_usage(&token)?;
    if resp.status == 401 {
        token = refresh(Some(&token))?;
        resp = get_usage(&token)?;
    }
    match resp.status {
        200 => {}
        401 | 403 => return Err(FetchError::LoginExpired),
        429 => return Err(FetchError::RateLimited),
        s => return Err(FetchError::Other(format!("Usage request failed (HTTP {s})"))),
    }
    let v: Value = serde_json::from_slice(&resp.body)
        .map_err(|_| FetchError::Other("Bad usage response".into()))?;
    let limit = |key: &str| -> Option<Limit> {
        let o = v.get(key)?;
        Some(Limit {
            percent: o.get("utilization")?.as_f64()?,
            resets_at: o.get("resets_at").and_then(Value::as_str).and_then(parse_iso8601),
        })
    };
    Ok(Usage { five_hour: limit("five_hour"), weekly: limit("seven_day") })
}

/// Parses e.g. "2026-11-11T14:10:00.320228+00:00" into unix seconds.
fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let offset = match b.get(i) {
        Some(b'+') | Some(b'-') => {
            let sign = if b[i] == b'-' { -1 } else { 1 };
            sign * (num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60)
        }
        _ => 0,
    };
    // Days from civil (Howard Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + se - offset)
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso() {
        assert_eq!(super::parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(super::parse_iso8601("2026-11-11T14:10:00.320228+00:00"), Some(1794406200));
        assert_eq!(super::parse_iso8601("2026-11-11T16:10:00+02:00"), Some(1794406200));
    }
}
