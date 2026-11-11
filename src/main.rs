#![windows_subsystem = "windows"]

mod api;
mod http;
mod render;

use http::wide;
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::time::{SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::w;

const WM_APP_RESULT: u32 = WM_APP + 1;
const T_LAYOUT: usize = 1; // keeps the widget attached and positioned
const T_FETCH: usize = 2; // one-shot: next usage fetch
const T_TICK: usize = 3; // refreshes tooltip countdowns

const ID_REFRESH: usize = 1;
const ID_OPEN: usize = 2;
const ID_AUTOSTART: usize = 3;
const ID_EXIT: usize = 4;
const ID_SHOW_LEFT: usize = 5;
const ID_HIDE_ICON: usize = 6;
const ID_FONT_BASE: usize = 200; // + index into FONT_SIZES
const ID_INTERVAL_BASE: usize = 100; // + index into INTERVALS

/// Update interval choices (seconds, menu label).
const INTERVALS: [(u32, &str); 7] = [
    (30, "30 seconds"),
    (60, "1 minute"),
    (120, "2 minutes"),
    (300, "5 minutes"),
    (600, "10 minutes"),
    (900, "15 minutes"),
    (1800, "30 minutes"),
];
const DEFAULT_INTERVAL: u32 = 120;

/// Font size choices (pixels at 100% scaling, menu label).
const FONT_SIZES: [(u32, &str); 6] = [
    (12, "Smallest"),
    (13, "Small"),
    (14, "Default"),
    (15, "Large"),
    (16, "Larger"),
    (18, "Largest"),
];
const DEFAULT_FONT_SIZE: u32 = 14;

const USAGE_URL: &str = "https://claude.ai/settings/usage";
const RUN_KEY: *const u16 = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: *const u16 = w!("ClaudeUsageWidget");
const SETTINGS_KEY: *const u16 = w!("Software\\ClaudeUsageWidget");

#[derive(Clone, Copy, PartialEq)]
struct RenderKey {
    h: i32,
    dpi: u32,
    dark: bool,
    hover: bool,
    five: Option<i64>,
    week: Option<i64>,
    stale: bool,
    show_left: bool,
    font_size: u32,
    hide_icon: bool,
}

struct State {
    ctl: HWND,
    widget: HWND,
    tip: HWND,
    taskbar: HWND,
    taskbar_created_msg: u32,
    usage: Option<api::Usage>,
    last_ok: Option<i64>,
    error: Option<String>,
    fetching: bool,
    hover: bool,
    dark: bool,
    rendered: Option<RenderKey>,
    pos: (i32, i32, i32, i32),
    /// Seconds between usage fetches.
    interval: u32,
    /// Show what's left instead of what's used.
    show_left: bool,
    /// Rate-limit errors in a row; each one doubles the wait.
    rate_limited: u32,
    /// Text size in pixels at 100% scaling.
    font_size: u32,
    /// Leave out the Claude mark.
    hide_icon: bool,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State {
        ctl: null_mut(), widget: null_mut(), tip: null_mut(), taskbar: null_mut(),
        taskbar_created_msg: 0, usage: None, last_ok: None, error: None,
        fetching: false, hover: false, dark: false, rendered: None, pos: (0, 0, 0, 0),
        interval: DEFAULT_INTERVAL, show_left: false, rate_limited: 0,
        font_size: DEFAULT_FONT_SIZE, hide_icon: false,
    });
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    ST.with(|s| f(&mut s.borrow_mut()))
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn main() {
    unsafe {
        // Single instance.
        CreateMutexW(null(), 0, w!("Local\\ClaudeUsageWidget.Singleton"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let hinst = GetModuleHandleW(null());
        let cursor = LoadCursorW(null_mut(), IDC_ARROW);
        for (name, proc_) in [
            (w!("ClaudeUsageCtl"), Some(ctl_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT)),
            (w!("ClaudeUsageWidget"), Some(widget_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT)),
        ] {
            let wc = WNDCLASSW {
                lpfnWndProc: proc_,
                hInstance: hinst,
                lpszClassName: name,
                hCursor: cursor,
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
        }
        let mut icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        };
        InitCommonControlsEx(&mut icc);

        // Hidden top-level window: owns timers and receives broadcasts
        // (TaskbarCreated, theme changes) that a taskbar child would not.
        let ctl = CreateWindowExW(
            WS_EX_TOOLWINDOW, w!("ClaudeUsageCtl"), w!("Claude Usage Widget"), WS_POPUP,
            0, 0, 0, 0, null_mut(), null_mut(), hinst, null(),
        );
        let msg = RegisterWindowMessageW(w!("TaskbarCreated"));
        with(|s| {
            s.ctl = ctl;
            s.taskbar_created_msg = msg;
            s.dark = system_dark();
            s.interval = read_setting("IntervalSeconds")
                .filter(|v| (30..=1800).contains(v))
                .unwrap_or(DEFAULT_INTERVAL);
            s.show_left = read_setting("ShowRemaining") == Some(1);
            s.font_size = read_setting("FontSize")
                .filter(|v| FONT_SIZES.iter().any(|(f, _)| f == v))
                .unwrap_or(DEFAULT_FONT_SIZE);
            s.hide_icon = read_setting("HideIcon") == Some(1);
        });
        ensure_widget();
        SetTimer(ctl, T_LAYOUT, 1000, None);
        SetTimer(ctl, T_TICK, 30_000, None);
        start_fetch();

        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

// ---------- data ----------

fn start_fetch() {
    let ctl = with(|s| {
        if s.fetching {
            return None;
        }
        s.fetching = true;
        Some(s.ctl as isize)
    });
    let Some(ctl) = ctl else { return };
    unsafe { KillTimer(ctl as HWND, T_FETCH) };
    std::thread::spawn(move || {
        let r = Box::new(api::fetch());
        unsafe { PostMessageW(ctl as HWND, WM_APP_RESULT, 0, Box::into_raw(r) as LPARAM) };
    });
}

fn on_result(r: Result<api::Usage, api::FetchError>) {
    let next_secs = with(|s| {
        s.fetching = false;
        let limited = matches!(r, Err(api::FetchError::RateLimited));
        match r {
            Ok(u) => {
                s.usage = Some(u);
                s.last_ok = Some(now());
                s.error = None;
            }
            Err(e) => s.error = Some(e.message()),
        }
        if limited {
            s.rate_limited += 1;
        } else {
            s.rate_limited = 0;
        }
        backoff_secs(s.interval, s.rate_limited)
    });
    let ctl = with(|s| s.ctl);
    unsafe { SetTimer(ctl, T_FETCH, next_secs * 1000, None) };
    ensure_widget();
    update_tooltip();
}

/// Wait before the next fetch. Normally the user's interval. After rate-limit
/// errors in a row it doubles each time, up to 10 minutes (never below the
/// user's interval).
fn backoff_secs(interval: u32, rate_limited: u32) -> u32 {
    const MAX_BACKOFF: u32 = 600;
    let grown = interval.saturating_mul(1 << rate_limited.min(10));
    grown.min(MAX_BACKOFF).max(interval)
}

fn is_stale(s: &State) -> bool {
    s.error.is_some() && s.last_ok.map_or(true, |t| now() - t > 600)
}

// ---------- widget ----------

/// Creates the widget inside the taskbar if needed, positions it just left of
/// the tray area, and redraws it when anything visible changed.
fn ensure_widget() {
    unsafe {
        let taskbar = FindWindowW(w!("Shell_TrayWnd"), null());
        let (mut widget, old_taskbar) = with(|s| (s.widget, s.taskbar));
        if taskbar.is_null() {
            return;
        }
        if widget.is_null() || IsWindow(widget) == 0 || taskbar != old_taskbar {
            if !widget.is_null() && IsWindow(widget) != 0 {
                DestroyWindow(widget);
            }
            widget = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("ClaudeUsageWidget"), w!("Claude Usage"),
                WS_CHILD | WS_CLIPSIBLINGS,
                0, 0, 1, 1, taskbar, null_mut(), GetModuleHandleW(null()), null(),
            );
            if widget.is_null() {
                return;
            }
            let tip = create_tooltip(widget);
            with(|s| {
                s.widget = widget;
                s.taskbar = taskbar;
                s.tip = tip;
                s.rendered = None;
                s.pos = (0, 0, 0, 0);
            });
            update_tooltip();
        }

        let mut tb = std::mem::zeroed::<RECT>();
        GetWindowRect(taskbar, &mut tb);
        let tray = FindWindowExW(taskbar, null_mut(), w!("TrayNotifyWnd"), null());
        let mut tr = std::mem::zeroed::<RECT>();
        if tray.is_null() || GetWindowRect(tray, &mut tr) == 0 {
            tr = tb;
            tr.left = tb.right;
        }
        let h = tb.bottom - tb.top;
        let dpi = GetDpiForWindow(taskbar).max(96);
        let scale = dpi as f32 / 96.0;

        let (key, needs_render, model) = with(|s| {
            let u = s.usage.unwrap_or_default();
            let key = RenderKey {
                h,
                dpi,
                dark: s.dark,
                hover: s.hover,
                five: u.five_hour.map(|l| l.percent.round() as i64),
                week: u.weekly.map(|l| l.percent.round() as i64),
                stale: is_stale(s),
                show_left: s.show_left,
                font_size: s.font_size,
                hide_icon: s.hide_icon,
            };
            let model = render::Model {
                five: u.five_hour.map(|l| l.percent),
                week: u.weekly.map(|l| l.percent),
                stale: key.stale,
                hover: s.hover,
                dark: s.dark,
                show_left: s.show_left,
                font_size: s.font_size,
                hide_icon: s.hide_icon,
            };
            (key, s.rendered != Some(key), model)
        });

        let mut w = with(|s| s.pos.2);
        if needs_render {
            let frame = render::render(&model, h, scale);
            w = frame.w;
            present(widget, &frame);
            with(|s| s.rendered = Some(key));
        }

        let x = tr.left - tb.left - w - (2.0 * scale) as i32;
        let pos = (x.max(0), 0, w, h);
        let moved = with(|s| {
            let changed = s.pos != pos;
            s.pos = pos;
            changed
        });
        // Stay above the taskbar's own XAML content.
        let on_top = GetWindow(taskbar, GW_CHILD) == widget;
        if moved || !on_top || IsWindowVisible(widget) == 0 {
            SetWindowPos(widget, HWND_TOP, pos.0, pos.1, pos.2, pos.3,
                SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }
}

unsafe fn present(widget: HWND, f: &render::Frame) {
    let dc = CreateCompatibleDC(null_mut());
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: f.w,
            biHeight: -f.h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        },
        ..std::mem::zeroed()
    };
    let mut bits = null_mut();
    let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    std::ptr::copy_nonoverlapping(f.px.as_ptr(), bits as *mut [u8; 4], f.px.len());
    let old = SelectObject(dc, bmp);
    let size = SIZE { cx: f.w, cy: f.h };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    UpdateLayeredWindow(widget, null_mut(), null(), &size, dc, &src, 0, &blend, ULW_ALPHA);
    SelectObject(dc, old);
    DeleteObject(bmp);
    DeleteDC(dc);
}

unsafe fn create_tooltip(widget: HWND) -> HWND {
    let tip = CreateWindowExW(
        WS_EX_TOPMOST, TOOLTIPS_CLASSW, null(),
        WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
        CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT, CW_USEDEFAULT,
        widget, null_mut(), GetModuleHandleW(null()), null(),
    );
    let mut text = wide("Claude usage");
    let ti = tool_info(widget, text.as_mut_ptr());
    SendMessageW(tip, TTM_ADDTOOLW, 0, &ti as *const _ as LPARAM);
    SendMessageW(tip, TTM_SETMAXTIPWIDTH, 0, 600);
    SendMessageW(tip, TTM_SETDELAYTIME, TTDT_AUTOPOP as usize, 30_000);
    tip
}

fn tool_info(widget: HWND, text: *mut u16) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: widget,
        uId: widget as usize,
        lpszText: text,
        ..unsafe { std::mem::zeroed() }
    }
}

fn update_tooltip() {
    let (tip, widget, text) = with(|s| (s.tip, s.widget, tooltip_text(s)));
    if tip.is_null() {
        return;
    }
    let mut t = wide(&text);
    let ti = tool_info(widget, t.as_mut_ptr());
    unsafe { SendMessageW(tip, TTM_UPDATETIPTEXTW, 0, &ti as *const _ as LPARAM) };
}

fn tooltip_text(s: &State) -> String {
    let mut out = String::from("Claude usage");
    let line = |name: &str, l: Option<api::Limit>, with_day: bool| -> String {
        match l {
            None => format!("\n{name}: –"),
            Some(l) => {
                let used = l.percent.round().clamp(0.0, 100.0) as i64;
                let mut t = format!("\n{name}: {used}% used, {}% left", 100 - used);
                if let Some(at) = l.resets_at {
                    t += &format!(
                        " · resets in {} ({})",
                        countdown(at - now()),
                        local_time(at, with_day)
                    );
                }
                t
            }
        }
    };
    let u = s.usage.unwrap_or_default();
    if s.usage.is_some() {
        out += &line("5-hour", u.five_hour, false);
        out += &line("Weekly", u.weekly, true);
    }
    if let Some(t) = s.last_ok {
        out += &format!("\nUpdated {}", local_time(t, false));
    }
    if let Some(e) = &s.error {
        out += &format!("\n⚠ {e}");
    } else if s.usage.is_none() {
        out += "\nLoading…";
    }
    out
}

fn countdown(secs: i64) -> String {
    let m = (secs.max(0) + 59) / 60;
    let (d, h, mm) = (m / 1440, (m % 1440) / 60, m % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {mm}m")
    } else {
        format!("{mm}m")
    }
}

fn local_time(unix: i64, with_day: bool) -> String {
    unsafe {
        let ft_val = (unix + 11_644_473_600) as u64 * 10_000_000;
        let ft = FILETIME { dwLowDateTime: ft_val as u32, dwHighDateTime: (ft_val >> 32) as u32 };
        let mut utc = std::mem::zeroed();
        let mut local = std::mem::zeroed();
        FileTimeToSystemTime(&ft, &mut utc);
        SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local);
        let mut buf = [0u16; 64];
        let mut out = String::new();
        if with_day {
            let n = GetDateFormatEx(null(), 0, &local, w!("ddd"), buf.as_mut_ptr(), 64, null());
            if n > 1 {
                out = String::from_utf16_lossy(&buf[..n as usize - 1]) + " ";
            }
        }
        let n = GetTimeFormatEx(null(), TIME_NOSECONDS, &local, null(), buf.as_mut_ptr(), 64);
        if n > 1 {
            out += &String::from_utf16_lossy(&buf[..n as usize - 1]);
        }
        out
    }
}

// ---------- settings ----------

fn system_dark() -> bool {
    let mut v: u32 = 1;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD, null_mut(), &mut v as *mut u32 as *mut _, &mut size,
        )
    };
    r == ERROR_SUCCESS && v == 0
}

fn read_setting(name: &str) -> Option<u32> {
    let mut v: u32 = 0;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, SETTINGS_KEY, wide(name).as_ptr(), RRF_RT_REG_DWORD,
            null_mut(), &mut v as *mut u32 as *mut _, &mut size)
    };
    (r == ERROR_SUCCESS).then_some(v)
}

fn write_setting(name: &str, v: u32) {
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, SETTINGS_KEY, wide(name).as_ptr(), REG_DWORD,
            &v as *const u32 as *const _, 4);
    }
}

fn set_interval(secs: u32) {
    let (ctl, fetching) = with(|s| {
        s.interval = secs;
        (s.ctl, s.fetching)
    });
    write_setting("IntervalSeconds", secs);
    // Apply right away: fetch now, then keep the new pace.
    if !fetching {
        start_fetch();
    } else {
        unsafe { SetTimer(ctl, T_FETCH, secs * 1000, None) };
    }
}

fn autostart_enabled() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ,
            null_mut(), null_mut(), &mut size) == ERROR_SUCCESS
    }
}

fn set_autostart(on: bool) {
    unsafe {
        if on {
            let exe = std::env::current_exe().unwrap_or_default();
            let v = wide(&format!("\"{}\"", exe.display()));
            RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, REG_SZ,
                v.as_ptr() as *const _, (v.len() * 2) as u32);
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}

fn open_usage_page() {
    unsafe {
        ShellExecuteW(null_mut(), w!("open"), wide(USAGE_URL).as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}

unsafe fn show_menu() {
    let ctl = with(|s| s.ctl);
    let menu = CreatePopupMenu();
    AppendMenuW(menu, MF_STRING, ID_REFRESH, w!("Refresh now"));
    AppendMenuW(menu, MF_STRING, ID_OPEN, w!("Open usage page"));
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    let (interval, show_left) = with(|s| (s.interval, s.show_left));
    let sub = CreatePopupMenu();
    for (i, (secs, label)) in INTERVALS.iter().enumerate() {
        let f = if *secs == interval { MF_STRING | MF_CHECKED } else { MF_STRING };
        AppendMenuW(sub, f, ID_INTERVAL_BASE + i, wide(label).as_ptr());
    }
    AppendMenuW(menu, MF_POPUP, sub as usize, w!("Update every"));
    let check = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
    AppendMenuW(menu, check(show_left), ID_SHOW_LEFT, w!("Show remaining instead of used"));
    let (font_size, hide_icon) = with(|s| (s.font_size, s.hide_icon));
    let custom = CreatePopupMenu();
    let fonts = CreatePopupMenu();
    for (i, (px, label)) in FONT_SIZES.iter().enumerate() {
        AppendMenuW(fonts, check(*px == font_size), ID_FONT_BASE + i, wide(label).as_ptr());
    }
    AppendMenuW(custom, MF_POPUP, fonts as usize, w!("Font size"));
    AppendMenuW(custom, check(hide_icon), ID_HIDE_ICON, w!("Hide Claude icon"));
    AppendMenuW(menu, MF_POPUP, custom as usize, w!("Customize"));
    AppendMenuW(menu, check(autostart_enabled()), ID_AUTOSTART, w!("Start with Windows"));
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Exit"));
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    SetForegroundWindow(ctl);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        pt.x, pt.y, 0, ctl, null());
    PostMessageW(ctl, WM_NULL, 0, 0);
    DestroyMenu(menu);
    match cmd as usize {
        ID_REFRESH => start_fetch(),
        ID_OPEN => open_usage_page(),
        ID_AUTOSTART => set_autostart(!autostart_enabled()),
        ID_SHOW_LEFT => {
            let on = with(|s| {
                s.show_left = !s.show_left;
                s.show_left
            });
            write_setting("ShowRemaining", on as u32);
            ensure_widget();
        }
        ID_HIDE_ICON => {
            let on = with(|s| {
                s.hide_icon = !s.hide_icon;
                s.hide_icon
            });
            write_setting("HideIcon", on as u32);
            ensure_widget();
        }
        id if (ID_FONT_BASE..ID_FONT_BASE + FONT_SIZES.len()).contains(&id) => {
            let px = FONT_SIZES[id - ID_FONT_BASE].0;
            with(|s| s.font_size = px);
            write_setting("FontSize", px);
            ensure_widget();
        }
        id if (ID_INTERVAL_BASE..ID_INTERVAL_BASE + INTERVALS.len()).contains(&id) => {
            set_interval(INTERVALS[id - ID_INTERVAL_BASE].0);
        }
        ID_EXIT => {
            let widget = with(|s| s.widget);
            if !widget.is_null() {
                DestroyWindow(widget);
            }
            DestroyWindow(ctl);
        }
        _ => {}
    }
}

// ---------- window procs ----------

unsafe extern "system" fn ctl_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let tb_msg = with(|s| s.taskbar_created_msg);
    match msg {
        WM_TIMER => {
            match wp {
                T_LAYOUT => ensure_widget(),
                T_FETCH => start_fetch(),
                T_TICK => {
                    update_tooltip();
                    // A window just reset: fetch now instead of waiting.
                    // Not while the last fetch failed: that would retry every
                    // 30 seconds and skip the backoff set in on_result.
                    let reset_passed = with(|s| {
                        if s.error.is_some() {
                            return false;
                        }
                        let u = s.usage.unwrap_or_default();
                        [u.five_hour, u.weekly].iter().flatten().any(|l| {
                            l.resets_at.is_some_and(|t| t < now() && s.last_ok.is_some_and(|ok| ok < t))
                        })
                    });
                    if reset_passed {
                        start_fetch();
                    }
                }
                _ => {}
            }
            0
        }
        WM_APP_RESULT => {
            let r = Box::from_raw(lp as *mut Result<api::Usage, api::FetchError>);
            on_result(*r);
            0
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            with(|s| s.dark = system_dark());
            ensure_widget();
            0
        }
        WM_POWERBROADCAST => {
            if wp as u32 == PBT_APMRESUMEAUTOMATIC {
                SetTimer(hwnd, T_FETCH, 5000, None);
            }
            1
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ if msg == tb_msg && msg != 0 => {
            // Explorer restarted: our old widget died with the old taskbar.
            with(|s| s.widget = null_mut());
            ensure_widget();
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn widget_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_MOUSEMOVE => {
            if !with(|s| std::mem::replace(&mut s.hover, true)) {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);
                ensure_widget();
            }
            0
        }
        WM_MOUSELEAVE => {
            with(|s| s.hover = false);
            ensure_widget();
            0
        }
        WM_LBUTTONUP => {
            open_usage_page();
            0
        }
        WM_RBUTTONUP => {
            show_menu();
            0
        }
        WM_DESTROY => {
            with(|s| {
                if s.widget == hwnd {
                    s.widget = null_mut();
                    s.tip = null_mut();
                }
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
