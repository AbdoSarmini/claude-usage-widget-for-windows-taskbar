#![windows_subsystem = "windows"]

mod api;
mod http;
mod render;
mod update;

use http::wide;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::Shell::{
    SHBrowseForFolderW, SHGetPathFromIDListW, ShellExecuteW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS,
    BROWSEINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::w;

const WM_APP_RESULT: u32 = WM_APP + 1;
const WM_APP_CSWAP: u32 = WM_APP + 2; // claude-swap finished; lparam: Box<Result<(), String>>
const WM_APP_UPDATE: u32 = WM_APP + 3; // update check or install finished; lparam: Box<UpdateMsg>
const T_LAYOUT: usize = 1; // keeps the widget attached and positioned
const T_ROLL: usize = 5; // animation frames while numbers roll
/// How long a changed number rolls.
const ROLL_SECS: f32 = 0.45;
const T_TICK: usize = 3; // refreshes tooltip countdowns
const T_UPDATE: usize = 4; // one-shot: next check for a new release
const T_FETCH_BASE: usize = 1000; // + account id, one-shot: that account's next fetch

const ID_REFRESH: usize = 1;
const ID_OPEN: usize = 2;
const ID_CSWAP: usize = 9;
const ID_UPDATE: usize = 10;
const ID_AUTOSTART: usize = 3;
const ID_EXIT: usize = 4;
const ID_SHOW_LEFT: usize = 5;
const ID_HIDE_ICON: usize = 6;
const ID_HIDE_LABELS: usize = 8;
const ID_ON_LEFT: usize = 11;
const ID_PIXEL_ICON: usize = 12;
const ID_RINGS: usize = 13;
const ID_RING_TEXT: usize = 14;
const ID_ROLL: usize = 15;
const ID_HIDE_FIVE: usize = 16;
const ID_HIDE_WEEK: usize = 17;
const ID_HIDE_FABLE: usize = 18;
const ID_CLAUDE_ICON: usize = 19;
const ID_ADD_ACCOUNT: usize = 7;
const ID_REMOVE_BASE: usize = 300; // + index into accounts
const ID_PER_COL_BASE: usize = 400; // + accounts per column (1 to 3)
const DEFAULT_PER_COL: u32 = 2;
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
use render::DEFAULT_FONT_SIZE;

const USAGE_URL: &str = "https://claude.ai/settings/usage";
const RUN_KEY: *const u16 = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: *const u16 = w!("ClaudeUsageWidget");
const SETTINGS_KEY: *const u16 = w!("Software\\ClaudeUsageWidget");

#[derive(Clone, PartialEq)]
struct RenderKey {
    h: i32,
    dpi: u32,
    dark: bool,
    hover: bool,
    /// Per account: rounded 5-hour %, weekly %, Fable %, stale.
    rows: Vec<(Option<i64>, Option<i64>, Option<i64>, bool, Option<u32>)>,
    show_left: bool,
    font_size: u32,
    hide_icon: bool,
    pixel_icon: bool,
    rings: bool,
    ring_text: bool,
    hide_labels: bool,
    hide_five: bool,
    hide_week: bool,
    hide_fable: bool,
    rows_per_col: u32,
}

/// One Claude Code login, found in its config folder.
struct Account {
    /// Stable id; also picks the account's fetch timer.
    id: u32,
    dir: PathBuf,
    /// Email, or the folder name until it's known.
    label: String,
    usage: Option<api::Usage>,
    /// While the numbers roll: the usage before the last update, and when it arrived.
    prev: Option<(api::Usage, Instant)>,
    last_ok: Option<i64>,
    error: Option<String>,
    fetching: bool,
    /// Rate-limit errors in a row; each one doubles the wait.
    rate_limited: u32,
}

impl Account {
    fn new(id: u32, dir: PathBuf) -> Account {
        let label = api::account_label(&dir);
        Account { id, dir, label, usage: None, prev: None, last_ok: None, error: None, fetching: false, rate_limited: 0 }
    }

    fn is_stale(&self) -> bool {
        self.error.is_some() && self.last_ok.map_or(true, |t| now() - t > 600)
    }
}

struct State {
    ctl: HWND,
    widget: HWND,
    tip: HWND,
    taskbar: HWND,
    taskbar_created_msg: u32,
    accounts: Vec<Account>,
    next_id: u32,
    hover: bool,
    dark: bool,
    rendered: Option<RenderKey>,
    pos: (i32, i32, i32, i32),
    /// Seconds between usage fetches.
    interval: u32,
    /// Show what's left instead of what's used.
    show_left: bool,
    /// Text size in pixels at 100% scaling.
    font_size: u32,
    /// Leave out the Claude mark.
    hide_icon: bool,
    /// Leave out the `5h` / `W` labels.
    hide_labels: bool,
    /// Sit at the taskbar's left end instead of next to the tray.
    on_left: bool,
    /// Pixel-art mark instead of the asterisk.
    pixel_icon: bool,
    /// A progress ring for each limit, and whether its number sits inside it.
    rings: bool,
    ring_text: bool,
    /// Roll the digits when a number changes.
    roll: bool,
    /// Limits turned off in "Show/hide limits". The 5-hour and Fable groups
    /// also hide on their own for plans without them.
    hide_five: bool,
    hide_week: bool,
    hide_fable: bool,
    /// Accounts stacked in each column of the widget.
    rows_per_col: u32,
    /// claude-swap's `cswap.exe`, found once at launch.
    cswap: Option<PathBuf>,
    /// A `cswap switch` is running.
    cswap_running: bool,
    /// A newer release, once a check found one.
    update: Option<update::Release>,
    /// What the updater is doing, shown greyed out in the menu.
    update_busy: Option<&'static str>,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State {
        ctl: null_mut(), widget: null_mut(), tip: null_mut(), taskbar: null_mut(),
        taskbar_created_msg: 0, accounts: Vec::new(), next_id: 0,
        hover: false, dark: false, rendered: None, pos: (0, 0, 0, 0),
        interval: DEFAULT_INTERVAL, show_left: false,
        font_size: DEFAULT_FONT_SIZE, hide_icon: false, hide_labels: false, on_left: false, pixel_icon: false, rings: false, ring_text: false, roll: false, hide_five: false, hide_week: false, hide_fable: false, rows_per_col: DEFAULT_PER_COL,
        cswap: None, cswap_running: false, update: None, update_busy: None,
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
        // Single instance. Right after an update the previous copy may still
        // be exiting, so the new one waits for it instead of quitting.
        let after_update = std::env::args().any(|a| a == update::UPDATED_ARG);
        let mut tries = 0;
        loop {
            let m = CreateMutexW(null(), 0, w!("Local\\ClaudeUsageWidget.Singleton"));
            if GetLastError() != ERROR_ALREADY_EXISTS {
                break;
            }
            CloseHandle(m);
            tries += 1;
            if !after_update || tries > 50 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        if let Ok(exe) = std::env::current_exe() {
            std::thread::spawn(move || update::clean_up_old(&exe));
        }
        // The folder picker needs COM on this thread.
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
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
            s.hide_labels = read_setting("HideLabels") == Some(1);
            s.on_left = read_setting("OnLeft") == Some(1);
            s.pixel_icon = read_setting("PixelIcon") == Some(1);
            s.rings = read_setting("Rings") == Some(1);
            s.ring_text = read_setting("RingText") == Some(1);
            s.roll = read_setting("RollNumbers") == Some(1);
            s.hide_five = read_setting("HideFiveHour") == Some(1);
            s.hide_week = read_setting("HideWeekly") == Some(1);
            s.hide_fable = read_setting("HideFable") == Some(1);
            s.cswap = find_cswap();
            s.rows_per_col = read_setting("AccountsPerColumn")
                .filter(|v| (1..=render::MAX_ROWS_PER_COLUMN as u32).contains(v))
                .unwrap_or(DEFAULT_PER_COL);
            for dir in load_account_dirs() {
                let id = s.next_id;
                s.next_id += 1;
                s.accounts.push(Account::new(id, dir));
            }
        });
        ensure_widget();
        SetTimer(ctl, T_LAYOUT, 1000, None);
        SetTimer(ctl, T_TICK, 30_000, None);
        SetTimer(ctl, T_UPDATE, UPDATE_FIRST_CHECK_MS, None);
        start_fetch_all();

        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

// ---------- data ----------

fn start_fetch(id: u32) {
    let job = with(|s| {
        let a = s.accounts.iter_mut().find(|a| a.id == id)?;
        if a.fetching {
            return None;
        }
        a.fetching = true;
        Some((s.ctl as isize, a.dir.clone()))
    });
    let Some((ctl, dir)) = job else { return };
    unsafe { KillTimer(ctl as HWND, T_FETCH_BASE + id as usize) };
    std::thread::spawn(move || {
        let r = api::fetch(&dir);
        let label = api::account_label(&dir);
        let msg = Box::new(FetchResult { id, label, r });
        unsafe { PostMessageW(ctl as HWND, WM_APP_RESULT, 0, Box::into_raw(msg) as LPARAM) };
    });
}

fn start_fetch_all() {
    for id in with(|s| s.accounts.iter().map(|a| a.id).collect::<Vec<_>>()) {
        start_fetch(id);
    }
}

struct FetchResult {
    id: u32,
    label: String,
    r: Result<api::Usage, api::FetchError>,
}

fn on_result(res: FetchResult) {
    let next = with(|s| {
        let interval = s.interval;
        let roll = s.roll;
        // Ignore results for an account removed while it was fetching.
        let a = s.accounts.iter_mut().find(|a| a.id == res.id)?;
        a.fetching = false;
        a.label = res.label;
        let limited = matches!(res.r, Err(api::FetchError::RateLimited));
        match res.r {
            Ok(u) => {
                a.prev = a.usage.filter(|_| roll).map(|old| (old, Instant::now()));
                a.usage = Some(u);
                a.last_ok = Some(now());
                a.error = None;
            }
            Err(e) => a.error = Some(e.message()),
        }
        if limited {
            a.rate_limited += 1;
        } else {
            a.rate_limited = 0;
        }
        Some((s.ctl, backoff_secs(interval, a.rate_limited)))
    });
    if let Some((ctl, secs)) = next {
        unsafe {
            SetTimer(ctl, T_FETCH_BASE + res.id as usize, secs * 1000, None);
            SetTimer(ctl, T_ROLL, 15, None); // stops itself once nothing rolls
        }
    }
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
            let rows: Vec<render::Row> = s
                .accounts
                .iter()
                .map(|a| {
                    let u = a.usage.unwrap_or_default();
                    render::Row {
                        five: u.five_hour.map(|l| l.percent),
                        week: u.weekly.map(|l| l.percent),
                        fable: u.fable.map(|l| l.percent),
                        roll: a.prev.and_then(|(o, at)| {
                            let t = at.elapsed().as_secs_f32() / ROLL_SECS;
                            let pct = |l: Option<api::Limit>| l.map(|l| l.percent);
                            (t < 1.0).then_some(([pct(o.five_hour), pct(o.weekly), pct(o.fable)], t))
                        }),
                        stale: a.is_stale(),
                    }
                })
                .collect();
            // Plans without a 5-hour limit (Ultra) report none: hide the group
            // on its own once every loaded account says so.
            let mut loaded = s.accounts.iter().filter_map(|a| a.usage).peekable();
            let hide_five = s.hide_five || (loaded.peek().is_some() && loaded.all(|u| !u.has_five_hour));
            let key = RenderKey {
                h,
                dpi,
                dark: s.dark,
                hover: s.hover,
                rows: rows
                    .iter()
                    .map(|r| {
                        let pct = |v: Option<f64>| v.map(|p| p.round() as i64);
                        (pct(r.five), pct(r.week), pct(r.fable), r.stale, r.roll.map(|(_, t)| (t * 1000.0) as u32))
                    })
                    .collect(),
                show_left: s.show_left,
                font_size: s.font_size,
                hide_icon: s.hide_icon,
                pixel_icon: s.pixel_icon,
                rings: s.rings,
                ring_text: s.ring_text,
                hide_labels: s.hide_labels,
                hide_five,
                hide_week: s.hide_week,
                hide_fable: s.hide_fable,
                rows_per_col: s.rows_per_col,
            };
            let model = render::Model {
                rows,
                hover: s.hover,
                dark: s.dark,
                show_left: s.show_left,
                font_size: s.font_size,
                hide_icon: s.hide_icon,
                pixel_icon: s.pixel_icon,
                rings: s.rings,
                ring_text: s.ring_text,
                hide_labels: s.hide_labels,
                hide_five,
                hide_week: s.hide_week,
                hide_fable: s.hide_fable,
                rows_per_col: s.rows_per_col as usize,
            };
            let changed = s.rendered.as_ref() != Some(&key);
            (key, changed, model)
        });

        let mut w = with(|s| s.pos.2);
        if needs_render {
            let frame = render::render(&model, h, scale);
            w = frame.w;
            present(widget, &frame);
            with(|s| s.rendered = Some(key));
        }

        let gap = (2.0 * scale) as i32;
        // The left end is free on a centered taskbar (unless the Widgets button is on).
        let x = if with(|s| s.on_left) { gap } else { tr.left - tb.left - w - gap };
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
    let multi = s.accounts.len() > 1;
    let indent = if multi { "  " } else { "" };
    let line = |name: &str, l: Option<api::Limit>, with_day: bool| -> String {
        match l {
            None => format!("\n{indent}{name}: –"),
            Some(l) => {
                let used = l.percent.round().clamp(0.0, 100.0) as i64;
                let mut t = format!("\n{indent}{name}: {used}% used, {}% left", 100 - used);
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
    for (i, a) in s.accounts.iter().enumerate() {
        if multi {
            // Say where the account sits: columns top to bottom, then left to right.
            let per = s.rows_per_col as usize;
            out += &format!("\n\n{}. {}", i + 1, a.label);
            if s.accounts.len() > per {
                out += &format!(" (column {}, row {})", i / per + 1, i % per + 1);
            }
        }
        let u = a.usage.unwrap_or_default();
        if a.usage.is_some() {
            if u.has_five_hour {
                out += &line("5-hour", u.five_hour, false);
            }
            out += &line("Weekly", u.weekly, true);
            if u.fable.is_some() {
                out += &line("Fable (weekly)", u.fable, true);
            }
        }
        if let Some(t) = a.last_ok {
            out += &format!("\n{indent}Updated {}", local_time(t, false));
        }
        if let Some(e) = &a.error {
            out += &format!("\n{indent}⚠ {e}");
        } else if a.usage.is_none() {
            out += &format!("\n{indent}Loading…");
        }
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
    with(|s| s.interval = secs);
    write_setting("IntervalSeconds", secs);
    // Apply right away: fetch now, then keep the new pace. Accounts that are
    // mid-fetch pick up the new pace when they finish.
    start_fetch_all();
}

fn read_string_setting(name: &str) -> Option<String> {
    let name = wide(name);
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, SETTINGS_KEY, name.as_ptr(), RRF_RT_REG_SZ,
            null_mut(), null_mut(), &mut size) != ERROR_SUCCESS
        {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2 + 1];
        size = (buf.len() * 2) as u32;
        if RegGetValueW(HKEY_CURRENT_USER, SETTINGS_KEY, name.as_ptr(), RRF_RT_REG_SZ,
            null_mut(), buf.as_mut_ptr() as *mut _, &mut size) != ERROR_SUCCESS
        {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

fn write_string_setting(name: &str, v: &str) {
    let v = wide(v);
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, SETTINGS_KEY, wide(name).as_ptr(), REG_SZ,
            v.as_ptr() as *const _, (v.len() * 2) as u32);
    }
}

// ---------- accounts ----------

/// Saved account folders, separated by `|` (which paths can't contain).
/// With nothing saved, the one account Claude Code uses by default.
fn load_account_dirs() -> Vec<PathBuf> {
    let dirs: Vec<PathBuf> = read_string_setting("Accounts")
        .unwrap_or_default()
        .split('|')
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .collect();
    if dirs.is_empty() {
        vec![api::default_dir()]
    } else {
        dirs
    }
}

fn save_accounts() {
    let joined = with(|s| {
        s.accounts.iter().map(|a| a.dir.display().to_string()).collect::<Vec<_>>().join("|")
    });
    write_string_setting("Accounts", &joined);
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    norm(a) == norm(b)
}

unsafe fn message(text: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let ctl = with(|s| s.ctl);
    MessageBoxW(ctl, wide(text).as_ptr(), w!("Claude Usage Widget"), flags | MB_SETFOREGROUND)
}

/// Asks for a folder with the standard Windows folder picker.
unsafe fn pick_folder() -> Option<PathBuf> {
    let ctl = with(|s| s.ctl);
    let title = wide(
        "Select the account's Claude Code folder (the one CLAUDE_CONFIG_DIR points to, \
         e.g. .claude-work), or the user folder that contains .claude.",
    );
    let bi = BROWSEINFOW {
        hwndOwner: ctl,
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        ..std::mem::zeroed()
    };
    let pidl = SHBrowseForFolderW(&bi);
    if pidl.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let ok = SHGetPathFromIDListW(pidl, buf.as_mut_ptr()) != 0;
    CoTaskMemFree(pidl as *const _);
    if !ok {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

unsafe fn add_account() {
    let Some(picked) = pick_folder() else { return };
    // Accept the config folder itself, or a home folder holding `.claude`.
    let dir = if api::has_login(&picked) {
        picked
    } else if api::has_login(&picked.join(".claude")) {
        picked.join(".claude")
    } else {
        let text = format!(
            "No Claude Code login found in\n{}\n\nAdd it anyway? It will show up once you sign in \
             to Claude Code with this folder.",
            picked.display()
        );
        if message(&text, MB_YESNO | MB_ICONWARNING) != IDYES {
            return;
        }
        picked
    };
    if with(|s| s.accounts.iter().any(|a| same_dir(&a.dir, &dir))) {
        message("This account is already in the widget.", MB_OK | MB_ICONINFORMATION);
        return;
    }
    let id = with(|s| {
        let id = s.next_id;
        s.next_id += 1;
        s.accounts.push(Account::new(id, dir));
        id
    });
    save_accounts();
    ensure_widget();
    update_tooltip();
    start_fetch(id);
}

unsafe fn remove_account(index: usize) {
    let Some(label) = with(|s| s.accounts.get(index).map(|a| a.label.clone())) else { return };
    let text = format!("Remove {label} from the widget?\n\nThis doesn't sign it out of Claude Code.");
    if message(&text, MB_YESNO | MB_ICONQUESTION) != IDYES {
        return;
    }
    let (ctl, id) = with(|s| {
        let a = s.accounts.remove(index);
        (s.ctl, a.id)
    });
    KillTimer(ctl, T_FETCH_BASE + id as usize);
    save_accounts();
    ensure_widget();
    update_tooltip();
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

// ---------- claude-swap ----------

/// Finds claude-swap's `cswap.exe`: on PATH, or in `~\.localin`, where
/// `uv tool install` and `pipx` put it.
fn find_cswap() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Ok(home) = std::env::var("USERPROFILE") {
        dirs.push(PathBuf::from(home).join(".local").join("bin"));
    }
    dirs.into_iter().map(|d| d.join("cswap.exe")).find(|p| p.is_file())
}

/// Runs `cswap switch` (rotate to the next account) in the background, then
/// refreshes the widget. JSON mode never stops to ask a question.
fn run_cswap_switch(exe: PathBuf) {
    let ctl = with(|s| {
        if s.cswap_running {
            return None;
        }
        s.cswap_running = true;
        Some(s.ctl as isize)
    });
    let Some(ctl) = ctl else { return };
    std::thread::spawn(move || {
        let r = Box::new(cswap_switch(&exe));
        unsafe { PostMessageW(ctl as HWND, WM_APP_CSWAP, 0, Box::into_raw(r) as LPARAM) };
    });
}

fn cswap_switch(exe: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const TIMEOUT_SECS: u64 = 60;

    let mut child = Command::new(exe)
        .args(["switch", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Could not start cswap: {e}"))?;
    // Read the pipes on their own threads so a chatty cswap can't block on a full pipe.
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as _));
    let err = read(child.stderr.take().map(|p| Box::new(p) as _));
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed().as_secs() < TIMEOUT_SECS => {
                std::thread::sleep(std::time::Duration::from_millis(200))
            }
            _ => {
                let _ = child.kill();
                return Err(format!("cswap didn't finish within {TIMEOUT_SECS} seconds."));
            }
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let json: Option<serde_json::Value> = serde_json::from_str(stdout.trim()).ok();
    if let Some(v) = &json {
        if let Some(m) = v["error"]["message"].as_str() {
            return Err(m.to_string());
        }
        if v["switched"] == serde_json::Value::Bool(false) {
            let reason = v["reason"].as_str().unwrap_or("cswap didn't switch accounts.");
            return Err(reason.to_string());
        }
    }
    if status.success() {
        Ok(())
    } else {
        let detail = [stderr.trim(), stdout.trim()].into_iter().find(|t| !t.is_empty());
        Err(detail.unwrap_or("cswap failed.").chars().take(500).collect())
    }
}

// ---------- updates ----------

/// First check shortly after launch, then once a day.
const UPDATE_FIRST_CHECK_MS: u32 = 15_000;
const UPDATE_EVERY_MS: u32 = 24 * 60 * 60 * 1000;

enum UpdateMsg {
    Checked { manual: bool, r: Result<Option<update::Release>, String> },
    Installed { rel: update::Release, r: Result<(), update::InstallError> },
}

fn post_update(ctl: isize, msg: UpdateMsg) {
    unsafe { PostMessageW(ctl as HWND, WM_APP_UPDATE, 0, Box::into_raw(Box::new(msg)) as LPARAM) };
}

/// Looks for a newer release. `manual` (from the menu) also reports
/// "up to date" and errors; the daily check stays quiet.
fn start_update_check(manual: bool) {
    let ctl = with(|s| {
        if s.update_busy.is_some() {
            return None;
        }
        s.update_busy = Some("Checking for updates…");
        Some(s.ctl as isize)
    });
    let Some(ctl) = ctl else { return };
    std::thread::spawn(move || post_update(ctl, UpdateMsg::Checked { manual, r: update::check() }));
}

fn start_update_install() {
    let job = with(|s| {
        if s.update_busy.is_some() {
            return None;
        }
        let rel = s.update.clone()?;
        s.update_busy = Some("Updating…");
        Some((s.ctl as isize, rel))
    });
    let Some((ctl, rel)) = job else { return };
    std::thread::spawn(move || {
        let r = match std::env::current_exe() {
            Ok(exe) => update::install(&rel, &exe),
            Err(e) => Err(update::InstallError::Other(format!("Can't find the widget's exe: {e}"))),
        };
        post_update(ctl, UpdateMsg::Installed { rel, r });
    });
}

unsafe fn on_update_msg(msg: UpdateMsg) {
    let ctl = with(|s| {
        s.update_busy = None;
        s.ctl
    });
    match msg {
        UpdateMsg::Checked { manual, r } => {
            SetTimer(ctl, T_UPDATE, UPDATE_EVERY_MS, None);
            let current = concat!("v", env!("CARGO_PKG_VERSION"));
            match r {
                Ok(Some(rel)) => {
                    let tag = rel.tag.clone();
                    with(|s| s.update = Some(rel));
                    if manual {
                        let text = format!(
                            "Version {tag} is available (you have {current}).\n\nUpdate now? The widget restarts by itself."
                        );
                        if message(&text, MB_YESNO | MB_ICONINFORMATION) == IDYES {
                            start_update_install();
                        }
                    }
                }
                Ok(None) => {
                    with(|s| s.update = None);
                    if manual {
                        message(&format!("You have the latest version ({current})."), MB_OK | MB_ICONINFORMATION);
                    }
                }
                Err(e) => {
                    if manual {
                        message(&format!("Couldn't check for updates:\n\n{e}"), MB_OK | MB_ICONWARNING);
                    }
                }
            }
        }
        UpdateMsg::Installed { rel, r } => match r {
            Ok(()) => {
                let exe = std::env::current_exe().unwrap_or_default();
                if let Err(e) = std::process::Command::new(&exe).arg(update::UPDATED_ARG).spawn() {
                    message(
                        &format!("{} was installed but couldn't be started:\n\n{e}\n\nStart the widget again by hand.", rel.tag),
                        MB_OK | MB_ICONWARNING,
                    );
                }
                quit();
            }
            Err(update::InstallError::NoAccess) => {
                let folder = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.display().to_string()))
                    .unwrap_or_default();
                message(
                    &format!(
                        "The widget can't update itself in\n{folder}\nbecause that folder needs administrator rights.\n\n\
                         The download page will open so you can replace the exe by hand. Keeping the widget in a folder \
                         of your own, like %LOCALAPPDATA%\\Programs\\ClaudeUsageWidget, lets it update itself."
                    ),
                    MB_OK | MB_ICONINFORMATION,
                );
                open_url(&rel.page);
            }
            Err(update::InstallError::Other(e)) => {
                message(&format!("Updating to {} failed:\n\n{e}", rel.tag), MB_OK | MB_ICONWARNING);
            }
        },
    }
}

unsafe fn quit() {
    let (widget, ctl) = with(|s| (s.widget, s.ctl));
    if !widget.is_null() {
        DestroyWindow(widget);
    }
    DestroyWindow(ctl);
}

fn on_cswap_done(r: Result<(), String>) {
    with(|s| s.cswap_running = false);
    match r {
        // The live login changed: show the new account's numbers now.
        Ok(()) => start_fetch_all(),
        Err(e) => unsafe {
            message(&format!("Switching accounts with claude-swap failed:

{e}"), MB_OK | MB_ICONWARNING);
        },
    }
}

fn open_usage_page() {
    open_url(USAGE_URL);
}

fn open_url(url: &str) {
    unsafe {
        ShellExecuteW(null_mut(), w!("open"), wide(url).as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}

unsafe fn show_menu() {
    let ctl = with(|s| s.ctl);
    let menu = CreatePopupMenu();
    // Settings first, then the everyday actions next to Exit, closest to the
    // taskbar where the menu opens.
    let tick = |id: usize| if setting_checked(id) == Some(true) { MF_STRING | MF_CHECKED } else { MF_STRING };
    let sub = CreatePopupMenu();
    for (i, (_, label)) in INTERVALS.iter().enumerate() {
        AppendMenuW(sub, tick(ID_INTERVAL_BASE + i), ID_INTERVAL_BASE + i, wide(label).as_ptr());
    }
    AppendMenuW(menu, MF_POPUP, sub as usize, w!("Update every"));
    AppendMenuW(menu, tick(ID_SHOW_LEFT), ID_SHOW_LEFT, w!("Show remaining instead of used"));
    let custom = CreatePopupMenu();
    let fonts = CreatePopupMenu();
    for (i, (_, label)) in FONT_SIZES.iter().enumerate() {
        AppendMenuW(fonts, tick(ID_FONT_BASE + i), ID_FONT_BASE + i, wide(label).as_ptr());
    }
    AppendMenuW(custom, MF_POPUP, fonts as usize, w!("Font size"));
    let icon = CreatePopupMenu();
    AppendMenuW(icon, tick(ID_HIDE_ICON), ID_HIDE_ICON, w!("Hide icon"));
    AppendMenuW(icon, tick(ID_CLAUDE_ICON), ID_CLAUDE_ICON, w!("Show Claude icon"));
    AppendMenuW(icon, tick(ID_PIXEL_ICON), ID_PIXEL_ICON, w!("Show pixel Claude icon"));
    AppendMenuW(custom, MF_POPUP, icon as usize, w!("Icon"));
    AppendMenuW(custom, tick(ID_RINGS), ID_RINGS, w!("Show usage rings"));
    AppendMenuW(custom, tick(ID_RING_TEXT), ID_RING_TEXT, w!("Numbers inside the rings"));
    AppendMenuW(custom, tick(ID_ROLL), ID_ROLL, w!("Roll numbers when they change"));
    AppendMenuW(custom, tick(ID_HIDE_LABELS), ID_HIDE_LABELS, w!("Hide labels"));
    let limits = CreatePopupMenu();
    AppendMenuW(limits, tick(ID_HIDE_FIVE), ID_HIDE_FIVE, w!("5-hour"));
    AppendMenuW(limits, tick(ID_HIDE_WEEK), ID_HIDE_WEEK, w!("Weekly"));
    AppendMenuW(limits, tick(ID_HIDE_FABLE), ID_HIDE_FABLE, w!("Fable (weekly)"));
    AppendMenuW(custom, MF_POPUP, limits as usize, w!("Show/hide limits"));
    AppendMenuW(custom, tick(ID_ON_LEFT), ID_ON_LEFT, w!("Show on the left side"));
    // Only matters with more than one account.
    let n_accounts = with(|s| s.accounts.len());
    let per = CreatePopupMenu();
    for n in 1..=render::MAX_ROWS_PER_COLUMN {
        let text = wide(&n.to_string());
        AppendMenuW(per, tick(ID_PER_COL_BASE + n), ID_PER_COL_BASE + n, text.as_ptr());
    }
    let per_flags = if n_accounts > 1 { MF_POPUP } else { MF_POPUP | MF_GRAYED };
    AppendMenuW(custom, per_flags, per as usize, w!("Accounts per column"));
    AppendMenuW(menu, MF_POPUP, custom as usize, w!("Customize"));
    let accounts = CreatePopupMenu();
    AppendMenuW(accounts, MF_STRING, ID_ADD_ACCOUNT, w!("Add account…"));
    let labels = with(|s| s.accounts.iter().map(|a| a.label.clone()).collect::<Vec<_>>());
    if !labels.is_empty() {
        AppendMenuW(accounts, MF_SEPARATOR, 0, null());
    }
    // Keep at least one account.
    let removable = if labels.len() > 1 { MF_STRING } else { MF_STRING | MF_GRAYED };
    for (i, label) in labels.iter().enumerate() {
        let text = wide(&format!("Remove {}", label.replace('&', "&&")));
        AppendMenuW(accounts, removable, ID_REMOVE_BASE + i, text.as_ptr());
    }
    AppendMenuW(menu, MF_POPUP, accounts as usize, w!("Accounts"));
    AppendMenuW(menu, tick(ID_AUTOSTART), ID_AUTOSTART, w!("Start with Windows"));
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_REFRESH, w!("Refresh now"));
    AppendMenuW(menu, MF_STRING, ID_OPEN, w!("Open usage page"));
    // Only when claude-swap was installed at launch.
    let (cswap, running) = with(|s| (s.cswap.clone(), s.cswap_running));
    if cswap.is_some() {
        let (flags, text) = if running {
            (MF_STRING | MF_GRAYED, w!("Switching account…"))
        } else {
            (MF_STRING, w!("Switch to next account (cswap)"))
        };
        AppendMenuW(menu, flags, ID_CSWAP, text);
    }
    let (busy, new_tag) = with(|s| (s.update_busy, s.update.as_ref().map(|r| r.tag.clone())));
    let update_text = match (busy, &new_tag) {
        (Some(b), _) => b.to_string(),
        (None, Some(tag)) => format!("Update to {tag}"),
        (None, None) => "Check for updates".to_string(),
    };
    let update_flags = if busy.is_some() { MF_STRING | MF_GRAYED } else { MF_STRING };
    AppendMenuW(menu, update_flags, ID_UPDATE, wide(&update_text).as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Exit"));
    refresh_menu(menu);
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    SetForegroundWindow(ctl);
    // Settings apply in place and leave the menu open; see `menu_filter`.
    MENU.set(menu);
    let hook = SetWindowsHookExW(WH_MSGFILTER, Some(menu_filter), null_mut(), GetCurrentThreadId());
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        pt.x, pt.y, 0, ctl, null());
    UnhookWindowsHookEx(hook);
    MENU.set(null_mut());
    PostMessageW(ctl, WM_NULL, 0, 0);
    DestroyMenu(menu);
    // Enter on a setting still closes the menu and lands here.
    if apply_setting(cmd as usize) {
        return;
    }
    match cmd as usize {
        ID_REFRESH => start_fetch_all(),
        ID_ADD_ACCOUNT => add_account(),
        id if (ID_REMOVE_BASE..ID_REMOVE_BASE + labels.len()).contains(&id) => {
            remove_account(id - ID_REMOVE_BASE);
        }
        ID_OPEN => open_usage_page(),
        ID_CSWAP => {
            if let Some(exe) = cswap {
                run_cswap_switch(exe);
            }
        }
        ID_UPDATE => {
            if new_tag.is_some() {
                start_update_install();
            } else {
                start_update_check(true);
            }
        }
        ID_EXIT => quit(),
        _ => {}
    }
}

thread_local! {
    /// The context menu while it is open.
    static MENU: Cell<HMENU> = const { Cell::new(null_mut()) };
}

/// Whether a setting's menu item is ticked, or None for items that are not settings.
fn setting_checked(id: usize) -> Option<bool> {
    if id == ID_AUTOSTART {
        return Some(autostart_enabled());
    }
    with(|s| {
        Some(match id {
            ID_SHOW_LEFT => s.show_left,
            ID_RINGS => s.rings,
            ID_RING_TEXT => s.ring_text,
            ID_ROLL => s.roll,
            ID_HIDE_LABELS => s.hide_labels,
            ID_ON_LEFT => s.on_left,
            // Ticked limits are shown.
            ID_HIDE_FIVE => !s.hide_five,
            ID_HIDE_WEEK => !s.hide_week,
            ID_HIDE_FABLE => !s.hide_fable,
            ID_HIDE_ICON => s.hide_icon,
            ID_CLAUDE_ICON => !s.hide_icon && !s.pixel_icon,
            ID_PIXEL_ICON => !s.hide_icon && s.pixel_icon,
            id if (ID_PER_COL_BASE + 1..=ID_PER_COL_BASE + render::MAX_ROWS_PER_COLUMN).contains(&id) => {
                s.rows_per_col as usize == id - ID_PER_COL_BASE
            }
            id if (ID_FONT_BASE..ID_FONT_BASE + FONT_SIZES.len()).contains(&id) => {
                s.font_size == FONT_SIZES[id - ID_FONT_BASE].0
            }
            id if (ID_INTERVAL_BASE..ID_INTERVAL_BASE + INTERVALS.len()).contains(&id) => {
                s.interval == INTERVALS[id - ID_INTERVAL_BASE].0
            }
            _ => return None,
        })
    })
}

/// Applies a setting picked in the menu. Returns false for other items.
fn apply_setting(id: usize) -> bool {
    match id {
        ID_AUTOSTART => set_autostart(!autostart_enabled()),
        ID_SHOW_LEFT => {
            let on = with(|s| {
                s.show_left = !s.show_left;
                s.show_left
            });
            write_setting("ShowRemaining", on as u32);
            ensure_widget();
        }
        ID_RINGS => {
            let on = with(|s| {
                s.rings = !s.rings;
                s.rings
            });
            write_setting("Rings", on as u32);
            ensure_widget();
        }
        ID_RING_TEXT => {
            let on = with(|s| {
                s.ring_text = !s.ring_text;
                s.ring_text
            });
            write_setting("RingText", on as u32);
            ensure_widget();
        }
        ID_ROLL => {
            let on = with(|s| {
                s.roll = !s.roll;
                s.roll
            });
            write_setting("RollNumbers", on as u32);
        }
        ID_HIDE_FIVE => {
            let on = with(|s| {
                s.hide_five = !s.hide_five;
                s.hide_five
            });
            write_setting("HideFiveHour", on as u32);
            ensure_widget();
        }
        ID_HIDE_WEEK => {
            let on = with(|s| {
                s.hide_week = !s.hide_week;
                s.hide_week
            });
            write_setting("HideWeekly", on as u32);
            ensure_widget();
        }
        ID_HIDE_FABLE => {
            let on = with(|s| {
                s.hide_fable = !s.hide_fable;
                s.hide_fable
            });
            write_setting("HideFable", on as u32);
            ensure_widget();
        }
        ID_ON_LEFT => {
            let on = with(|s| {
                s.on_left = !s.on_left;
                s.on_left
            });
            write_setting("OnLeft", on as u32);
            ensure_widget();
        }
        ID_HIDE_LABELS => {
            let on = with(|s| {
                s.hide_labels = !s.hide_labels;
                s.hide_labels
            });
            write_setting("HideLabels", on as u32);
            ensure_widget();
        }
        id @ (ID_HIDE_ICON | ID_CLAUDE_ICON | ID_PIXEL_ICON) => {
            let (hide, pixel) = (id == ID_HIDE_ICON, id == ID_PIXEL_ICON);
            with(|s| {
                s.hide_icon = hide;
                // Hiding keeps the chosen mark for when it is shown again.
                if !hide {
                    s.pixel_icon = pixel;
                }
            });
            write_setting("HideIcon", hide as u32);
            if !hide {
                write_setting("PixelIcon", pixel as u32);
            }
            ensure_widget();
        }
        id if (ID_PER_COL_BASE + 1..=ID_PER_COL_BASE + render::MAX_ROWS_PER_COLUMN).contains(&id) => {
            let n = (id - ID_PER_COL_BASE) as u32;
            with(|s| s.rows_per_col = n);
            write_setting("AccountsPerColumn", n);
            ensure_widget();
            update_tooltip();
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
        _ => return false,
    }
    true
}

/// Brings the ticks up to date, and greys out "Numbers inside the rings" without rings.
unsafe fn refresh_menu(menu: HMENU) {
    for i in 0..GetMenuItemCount(menu) {
        let sub = GetSubMenu(menu, i);
        if !sub.is_null() {
            refresh_menu(sub);
            continue;
        }
        let id = GetMenuItemID(menu, i) as usize;
        if let Some(on) = setting_checked(id) {
            CheckMenuItem(menu, i as u32, MF_BYPOSITION | if on { MF_CHECKED } else { MF_UNCHECKED });
        }
        if id == ID_RING_TEXT {
            let on = with(|s| s.rings);
            EnableMenuItem(menu, i as u32, MF_BYPOSITION | if on { MF_ENABLED } else { MF_GRAYED });
        }
    }
}

/// Runs inside the menu's loop. A click on a setting applies it and updates
/// the open menu instead of closing it; any other click goes on as usual.
unsafe extern "system" fn menu_filter(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let m = &*(lp as *const MSG);
    let up = matches!(m.message, WM_LBUTTONUP | WM_RBUTTONUP);
    let dbl = matches!(m.message, WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK);
    if code == MSGF_MENU as i32 && (up || dbl) {
        let hwnd = WindowFromPoint(m.pt);
        let mut class = [0u16; 8];
        let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        // Open menus are windows of class "#32768".
        if String::from_utf16_lossy(&class[..n]) == "#32768" {
            let menu = SendMessageW(hwnd, MN_GETHMENU, 0, 0) as HMENU;
            let i = MenuItemFromPoint(null_mut(), menu, m.pt);
            let usable = i >= 0 && GetMenuState(menu, i as u32, MF_BYPOSITION) & (MF_GRAYED | MF_DISABLED) == 0;
            let id = if usable { GetMenuItemID(menu, i) as usize } else { 0 };
            if dbl && setting_checked(id).is_some() {
                // A quick second click arrives as a double-click, which the menu
                // takes as a choice and closes; its button-up toggles again.
                return 1;
            }
            if up && apply_setting(id) {
                refresh_menu(MENU.get());
                EnumThreadWindows(GetCurrentThreadId(), Some(repaint_menu), 0);
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wp, lp)
}

unsafe extern "system" fn repaint_menu(hwnd: HWND, _: LPARAM) -> BOOL {
    InvalidateRect(hwnd, null(), 1);
    1
}

// ---------- window procs ----------

unsafe extern "system" fn ctl_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let tb_msg = with(|s| s.taskbar_created_msg);
    match msg {
        WM_TIMER => {
            match wp {
                T_LAYOUT => ensure_widget(),
                T_ROLL => {
                    let rolling = with(|s| {
                        for a in &mut s.accounts {
                            a.prev = a.prev.filter(|(_, at)| at.elapsed().as_secs_f32() < ROLL_SECS);
                        }
                        s.accounts.iter().any(|a| a.prev.is_some())
                    });
                    if !rolling {
                        KillTimer(hwnd, T_ROLL);
                    }
                    ensure_widget(); // the next frame, or the settled numbers
                }
                T_UPDATE => {
                    KillTimer(hwnd, T_UPDATE);
                    start_update_check(false);
                }
                T_TICK => {
                    update_tooltip();
                    // A window just reset: fetch now instead of waiting.
                    // Not while the last fetch failed: that would retry every
                    // 30 seconds and skip the backoff set in on_result.
                    let reset_passed: Vec<u32> = with(|s| {
                        s.accounts
                            .iter()
                            .filter(|a| {
                                if a.error.is_some() {
                                    return false;
                                }
                                let u = a.usage.unwrap_or_default();
                                [u.five_hour, u.weekly, u.fable].iter().flatten().any(|l| {
                                    l.resets_at
                                        .is_some_and(|t| t < now() && a.last_ok.is_some_and(|ok| ok < t))
                                })
                            })
                            .map(|a| a.id)
                            .collect()
                    });
                    for id in reset_passed {
                        start_fetch(id);
                    }
                }
                id if id >= T_FETCH_BASE => start_fetch((id - T_FETCH_BASE) as u32),
                _ => {}
            }
            0
        }
        WM_APP_UPDATE => {
            let m = Box::from_raw(lp as *mut UpdateMsg);
            on_update_msg(*m);
            0
        }
        WM_APP_CSWAP => {
            let r = Box::from_raw(lp as *mut Result<(), String>);
            on_cswap_done(*r);
            0
        }
        WM_APP_RESULT => {
            let r = Box::from_raw(lp as *mut FetchResult);
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
                for id in with(|s| s.accounts.iter().map(|a| a.id).collect::<Vec<_>>()) {
                    SetTimer(hwnd, T_FETCH_BASE + id as usize, 5000, None);
                }
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
