# Claude Usage Widget For Windows Taskbar

See your Claude **5-hour** and **weekly** usage right on the Windows 11 taskbar, next to the clock.

<p>
  <img src="docs/screenshot-light.png" alt="Widget on a light taskbar" width="490"><br>
  <img src="docs/screenshot-dark.png" alt="Widget on a dark taskbar" width="490">
</p>

### ⬇️ [Download ClaudeUsageWidget.exe](https://github.com/AbdoSarmini/claude-usage-widget-for-windows-taskbar/releases/latest/download/ClaudeUsageWidget.exe)

One small file (~450 KB). No installer, no runtime, nothing else to download.

## Features

- **Always visible.** Shows `5h` and `W` (weekly) usage right in the taskbar, not hidden behind a tray icon.
- **Tiny.** About 5 MB of memory and practically zero CPU. It's built only on Windows' own APIs.
- **Colour warnings.** A number turns orange at 80% used and red at 95%.
- **Details on hover.** The tooltip shows used and left percentages, when each limit resets, and when the numbers were last updated.
- **Light and dark taskbars.** It follows your Windows theme automatically.
- **Show remaining.** Optionally shows what's left instead of what's used.
- **Adjustable update interval.** From 30 seconds to 30 minutes.
- **Multiple accounts.** Track several Claude Code logins at once, stacked in columns.
- **Customizable.** Six font sizes, and you can hide the Claude icon or the `5h` / `W` labels to make the widget narrower.
- **claude-swap support.** If [claude-swap](https://github.com/realiti4/claude-swap) is installed, switch to your next saved account from the menu.
- **Updates itself.** Checks GitHub for new versions and installs one with a single click, never without asking.

## Requirements

- **Windows 11.**
- **Claude Code signed in with a Claude subscription** (Pro, Max, Team or Enterprise).
  - The widget reads the login that Claude Code saves on your PC, so you don't sign in again.
  - If you haven't yet, [install Claude Code](https://code.claude.com/docs/en/setup), run `claude` once and sign in with your Claude account.
  - Claude Code doesn't need to stay open. The widget renews the login itself when it expires.

> API-key users have no 5-hour or weekly limits, so there's nothing to show. Using only the Claude desktop app or claude.ai (without Claude Code) isn't supported.

## Install

1. [Download `ClaudeUsageWidget.exe`](https://github.com/AbdoSarmini/claude-usage-widget-for-windows-taskbar/releases/latest/download/ClaudeUsageWidget.exe) and put it somewhere permanent, e.g. `%LOCALAPPDATA%\Programs\ClaudeUsageWidget\`. In a folder that needs administrator rights, like `C:\Program Files\`, the widget can't [update itself](#updates).
2. Double-click it. The widget appears to the left of the tray icons.
3. Right-click the widget and tick **Start with Windows** to have it start automatically.

> **"Windows protected your PC"?** The exe isn't code-signed, so SmartScreen may warn you the first time. Click **More info → Run anyway**. You can also [build it yourself](#build-from-source) from the source.

## Usage

<img src="docs/screenshot-menu.png" alt="The widget's right-click menu" width="377">

| Action | What it does |
|---|---|
| Hover | Usage details and reset times |
| Left-click | Opens your usage page on claude.ai |
| Right-click → **Update every** | 30 s, 1, 2, 5, 10, 15 or 30 minutes (default: 2 minutes) |
| Right-click → **Show remaining instead of used** | Shows `75%` left instead of `25%` used |
| Right-click → **Customize → Font size** | Six text sizes, from Smallest to Largest |
| Right-click → **Customize → Icon** | Choose **Hide icon** (numbers only), **Show Claude icon** or **Show pixel Claude icon**, and the **Icon size** |
| Right-click → **Customize → Hide labels** | Leaves out the `5h` / `W` / `F` labels and shows only the percentages, making the widget narrower |
| Right-click → **Customize → Show/hide limits** | Turns the 5-hour, weekly and Fable numbers on or off. The 5-hour group also hides on its own for accounts without a 5-hour limit (Ultra), and the Fable group for accounts without a Fable limit |
| Right-click → **Customize → Show usage rings** | Adds a progress ring next to each number |
| Right-click → **Customize → Numbers inside the rings** | Larger rings with the percentage inside (needs rings on) |
| Right-click → **Customize → Roll numbers when they change** | Changed digits roll to their new value with a little motion blur |
| Right-click → **Customize → Show on the left side** | Puts the widget at the taskbar's left end instead of next to the tray (for centered taskbars, or when something else sits by the tray) |
| Right-click → **Customize → Accounts per column** | 1, 2 or 3 accounts stacked per column (default: 2; needs more than one account) |
| Right-click → **Accounts → Add account…** | Adds another Claude Code login (see [Multiple accounts](#multiple-accounts)) |
| Right-click → **Accounts → Remove …** | Takes an account off the widget (doesn't sign it out) |
| Right-click → **Start with Windows** | Starts the widget when you sign in to Windows |
| Right-click → **Refresh now** | Updates immediately |
| Right-click → **Open usage page** | Opens your usage page on claude.ai |
| Right-click → **Switch to next account (cswap)** | Only shown when [claude-swap](https://github.com/realiti4/claude-swap) was installed when the widget started: runs `cswap switch` to make the next saved account the live Claude Code login, then updates the numbers |
| Right-click → **Check for updates** / **Update to vX.Y.Z** | Checks GitHub for a newer release; once one is found, one click installs it and restarts the widget (see [Updates](#updates)) |
| Right-click → **Exit** | Closes the widget |

The numbers update on the interval you choose, right after a limit resets, and when your PC wakes from sleep.

## Multiple accounts

If you run Claude Code with different config folders, for example a work account started like this:

```powershell
$env:CLAUDE_CONFIG_DIR = "$HOME\.claude-work"
claude
```

right-click the widget, choose **Accounts → Add account…** and select that folder (`.claude-work`). Picking a user folder that contains a `.claude` folder works too.

- Each account gets its own row: `5h` and `W` for that login.
- Accounts are stacked 2 per column by default; the next one starts a new column to the right. Change it under **Customize → Accounts per column** (1 to 3).
- Font size still applies with stacked accounts: the text fits the row height and grows or shrinks from there.
- The tooltip lists the accounts in the same order, top to bottom, then left to right, named by their email.
- Each account updates and backs off on its own, so one failing login doesn't hold up the others.

> The Claude desktop app keeps its login separately, so it can't be added. To track that account, sign in to Claude Code with it too.
>
> Don't add folders that hold copies of the same login file. Renewing one copy can sign out the others.

## Updates

The widget checks GitHub for a new release shortly after it starts and then once a day. It never updates without asking:

- When a newer version exists, the right-click menu shows **Update to vX.Y.Z**. Click it to download the new exe, check it against the SHA-256 checksum GitHub publishes for the release, swap it in, and restart. Settings and accounts are kept.
- **Check for updates** checks right away and tells you if you're up to date.
- If the widget sits in a folder that needs administrator rights, it opens the download page instead so you can replace the exe by hand.

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
| Widget overlaps taskbar app icons | This happens with many apps pinned, left-aligned taskbar icons, or several accounts side by side. The widget always sits left of the tray icons and grows to the left. To make it narrower, use **Customize → Hide labels**, **Icon → Hide icon**, a smaller **Font size**, or more **Accounts per column**. |
| Widget doesn't appear on my second monitor | Only the main taskbar is supported. |

## Privacy

- **Where your data goes:** only to Anthropic, the same servers Claude Code talks to.
  - `api.anthropic.com` to read your usage.
  - `platform.claude.com` to renew your login.
- **Update checks:** GitHub, to look for and download new releases of this widget (`api.github.com`, `github.com`). Nothing about you or your usage is sent.
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
| `src/update.rs` | Checking GitHub for new releases and installing them |

## Disclaimer

This is an unofficial community project. It isn't affiliated with or endorsed by Anthropic. "Claude" is a trademark of Anthropic.

## License

[MIT](LICENSE)
