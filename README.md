# Claude Usage Widget For Windows Taskbar

See your Claude **5-hour** and **weekly** usage right on the Windows 11 taskbar, next to the clock.

<p>
  <img src="docs/screenshot-light.png" alt="Widget on a light taskbar" width="490"><br>
  <img src="docs/screenshot-dark.png" alt="Widget on a dark taskbar" width="490">
</p>

### ⬇️ [Download ClaudeUsageWidget.exe](https://github.com/AbdoSarmini/claude-usage-widget-for-windows-taskbar/releases/latest/download/ClaudeUsageWidget.exe)

One small file (~300 KB). No installer, no runtime, nothing else to download.

## Features

- **Always visible.** Shows `5h` and `W` (weekly) usage right in the taskbar, not hidden behind a tray icon.
- **Tiny.** About 2–3 MB of memory and practically zero CPU. It's built only on Windows' own APIs.
- **Colour warnings.** A number turns orange at 80% used and red at 95%.
- **Details on hover.** The tooltip shows used and left percentages, when each limit resets, and when the numbers were last updated.
- **Light and dark taskbars.** It follows your Windows theme automatically.
- **Show remaining.** Optionally shows what's left instead of what's used.
- **Adjustable update interval.** From 30 seconds to 30 minutes.

## Requirements

- **Windows 11.**
- **Claude Code signed in with a Claude subscription** (Pro, Max, Team or Enterprise).
  - The widget reads the login that Claude Code saves on your PC, so you don't sign in again.
  - If you haven't yet, [install Claude Code](https://code.claude.com/docs/en/setup), run `claude` once and sign in with your Claude account.
  - Claude Code doesn't need to stay open. The widget renews the login itself when it expires.

> API-key users have no 5-hour or weekly limits, so there's nothing to show. Using only the Claude desktop app or claude.ai (without Claude Code) isn't supported.

## Install

1. [Download `ClaudeUsageWidget.exe`](https://github.com/AbdoSarmini/claude-usage-widget-for-windows-taskbar/releases/latest/download/ClaudeUsageWidget.exe) and put it somewhere permanent, e.g. `C:\Program Files\ClaudeUsageWidget\` or `%LOCALAPPDATA%\Programs\ClaudeUsageWidget\`.
2. Double-click it. The widget appears to the left of the tray icons.
3. Right-click the widget and tick **Start with Windows** to have it start automatically.

> **"Windows protected your PC"?** The exe isn't code-signed, so SmartScreen may warn you the first time. Click **More info → Run anyway**. You can also [build it yourself](#build-from-source) from the source.

## Usage

| Action | What it does |
|---|---|
| Hover | Usage details and reset times |
| Left-click | Opens your usage page on claude.ai |
| Right-click → **Refresh now** | Updates immediately |
| Right-click → **Update every** | 30 s, 1, 2, 5, 10, 15 or 30 minutes (default: 2 minutes) |
| Right-click → **Show remaining instead of used** | Shows `75%` left instead of `25%` used |
| Right-click → **Customize → Font size** | Six text sizes, from Smallest to Largest |
| Right-click → **Customize → Hide Claude icon** | Shows only the numbers, without the Claude mark |
| Right-click → **Start with Windows** | Starts the widget when you sign in to Windows |
| Right-click → **Exit** | Closes the widget |

The numbers update on the interval you choose, right after a limit resets, and when your PC wakes from sleep.

## Uninstall

1. Right-click the widget and untick **Start with Windows**.
2. Click **Exit**.
3. Delete the exe.

Settings are stored in `HKEY_CURRENT_USER\Software\ClaudeUsageWidget`. You can delete that key too.

## Troubleshooting

| Problem | Fix |
|---|---|
| Numbers are dimmed, tooltip says to sign in | Run `claude` in a terminal and sign in again. |
| `–` instead of numbers | No usage data yet: check your internet connection, or hover for the error. |
| Widget overlaps taskbar app icons | This happens with many apps pinned or with left-aligned taskbar icons. The widget always sits left of the tray icons. |
| Widget doesn't appear on my second monitor | Only the main taskbar is supported. |

## Privacy

- **Where your data goes:** only to Anthropic, the same servers Claude Code talks to.
  - `api.anthropic.com` to read your usage.
  - `platform.claude.com` to renew your login.
- **Your login:** it stays on your PC.
- **Nothing else:** no analytics, no telemetry, no other servers.

## How it works

- **Taskbar placement.** Windows 11 removed the official way to add things to the taskbar (deskbands). Instead, the widget is a small see-through window placed inside the taskbar, just left of the tray area. A light check once a second keeps it in place when the tray changes size, and re-attaches it if Explorer restarts.
- **Usage data.** Comes from `GET https://api.anthropic.com/api/oauth/usage`, the same endpoint Claude Code's `/usage` command uses. It's not a public API and could change.
- **Login renewal.** When the saved login is about to expire, the widget renews it the same way Claude Code does, and holds Claude Code's own lock while doing so. The two never renew at the same time, and both keep working.

## Build from source

You need [Rust](https://rustup.rs/) (MSVC toolchain) and the Windows SDK (installed with Visual Studio Build Tools).

```
git clone https://github.com/AbdoSarmini/claude-usage-widget-for-windows-taskbar
cd claude-usage-widget-for-windows-taskbar
cargo build --release
target\release\claude-usage-widget.exe
```

| File | What it contains |
|---|---|
| `src/main.rs` | Windows, taskbar placement, menu, tooltip, timers |
| `src/render.rs` | Drawing the widget (text, logo, dividers) |
| `src/api.rs` | Reading and renewing the login, fetching usage |
| `src/http.rs` | Small HTTPS client using Windows' built-in WinHTTP |

## Disclaimer

This is an unofficial community project. It isn't affiliated with or endorsed by Anthropic. "Claude" is a trademark of Anthropic.

## License

[MIT](LICENSE)
