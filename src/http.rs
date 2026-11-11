//! Minimal HTTPS client on top of WinHTTP (ships with Windows, no TLS crate needed).

use std::ffi::c_void;
use std::ptr::null;
use windows_sys::Win32::Networking::WinHttp::*;

pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn check(h: *mut c_void, what: &str) -> Result<Handle, String> {
    if h.is_null() {
        Err(format!("{what} failed ({})", last_error()))
    } else {
        Ok(Handle(h))
    }
}

fn last_error() -> u32 {
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

/// `headers` are CRLF-separated "Name: value" lines.
pub fn request(
    method: &str,
    host: &str,
    path: &str,
    headers: &str,
    body: Option<&[u8]>,
) -> Result<Response, String> {
    unsafe {
        let session = check(
            WinHttpOpen(
                wide("claude-usage-widget/1.0").as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                null(),
                null(),
                0,
            ),
            "WinHttpOpen",
        )?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 15_000);
        let conn = check(
            WinHttpConnect(session.0, wide(host).as_ptr(), 443, 0),
            "connect",
        )?;
        let req = check(
            WinHttpOpenRequest(
                conn.0,
                wide(method).as_ptr(),
                wide(path).as_ptr(),
                null(),
                null(),
                null(),
                WINHTTP_FLAG_SECURE,
            ),
            "open request",
        )?;
        let hdrs = wide(headers);
        let (bptr, blen) = match body {
            Some(b) => (b.as_ptr() as *const c_void, b.len() as u32),
            None => (null(), 0),
        };
        if WinHttpSendRequest(req.0, hdrs.as_ptr(), u32::MAX, bptr, blen, blen, 0) == 0 {
            return Err(format!("network error ({})", last_error()));
        }
        if WinHttpReceiveResponse(req.0, std::ptr::null_mut()) == 0 {
            return Err(format!("network error ({})", last_error()));
        }
        let mut status: u32 = 0;
        let mut size = 4u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut status as *mut u32 as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
        );
        let mut out = Vec::new();
        loop {
            let mut avail = 0u32;
            if WinHttpQueryDataAvailable(req.0, &mut avail) == 0 {
                return Err(format!("read error ({})", last_error()));
            }
            if avail == 0 {
                break;
            }
            let start = out.len();
            out.resize(start + avail as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(req.0, out[start..].as_mut_ptr() as *mut c_void, avail, &mut read) == 0 {
                return Err(format!("read error ({})", last_error()));
            }
            out.truncate(start + read as usize);
            if read == 0 {
                break;
            }
        }
        Ok(Response { status, body: out })
    }
}
