# Codenotch for Linux

A Linux port of [Codenotch](https://github.com/vinzdg/codenotch) — the usage notch that sits on the
edge of your screen and answers two questions at a glance: **how much of my AI allowance is left**,
and **is Claude still working**.

This is a fork of the [Windows port](https://github.com/Im-Midi/codenotch-windows) (Rust + Tauri 2),
which already kept every Win32 call behind `#[cfg(windows)]`. The Linux build replaces those with
GTK window hints, an X11 client (x11rb) and `/proc`, and swaps `cmd /C start`, `explorer`, the
registry Run key and `%APPDATA%` for `xdg-open`, an XDG autostart entry and `~/.config`. The
providers, session engine, hook messenger and the HTML pill are unchanged. Tested on Ubuntu 26.04,
GNOME, Wayland session (see [Wayland](#wayland) below).

## What it shows

| Cell | Source | How it reads it |
|---|---|---|
| **Claude** | `GET https://api.anthropic.com/api/oauth/usage` with the token Claude Code keeps in `~/.claude/.credentials.json` | Session / weekly windows, 429 back-off with a persisted deadline, stale readings dimmed with their age. A thin arc spins inside the ring while a Claude session is working, and pulses amber when one is waiting on you (Claude Code hooks + transcript watcher). |
| **Codex** | `GET https://chatgpt.com/backend-api/wham/usage` with the session Codex keeps in `~/.codex/auth.json` (read only, never refreshed), falling back to the `rate_limits` snapshot in the newest rollout log | Live primary/secondary windows while Codex is signed in; otherwise the last snapshot, marked stale by its own timestamp. |
| **Cursor** | The editor's own session from `~/.config/Cursor/User/globalStorage/state.vscdb` → `cursor.com/api/usage-summary` | Included usage / API usage / on-demand, reset at billing-cycle end. |
| **Antigravity** | The local `language_server` bridge (found with `ps` + `lsof`), then a plain count of today's model turns | Honest degradation: a percentage only when one exists, a `~count` when it does not. The licensed-account path reads the Windows Credential Manager and is not ported (Linux keeps the token in the Secret Service). |

Providers that are not installed simply do not get a cell.

## Install / build

Prerequisites: Rust (`rustup`), and the Tauri 2 Linux libraries:

```bash
sudo apt install build-essential libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev
```

Then, from the repository root:

```bash
cargo build --release
./install.sh                          # copies both binaries to ~/.local/bin and adds a launcher + icon
codenotch                             # pill appears on the right edge of the primary monitor
codenotch doctor                      # self-diagnosis: credentials, data sources, icons, hooks
codenotch install-hooks               # wires codenotch-hook into ~/.claude/settings.json (backup written first)
```

The hook can also be installed from the tray menu. Without it the transcript watcher still detects
sessions from `~/.claude/projects`; the hook adds the instant "waiting on you" signal.

Tray menu: refresh now, reset position, open data folder (`~/.config/codenotch` — logs, persisted
readings, icon overrides), start at sign-in (an XDG autostart entry), install/uninstall Claude Code
hooks. On GNOME the tray needs the AppIndicator extension, which Ubuntu ships enabled.

### Wayland

Wayland lets no application place its own window at a screen edge, and GNOME ignores keep-above
requests, so a native Wayland notch would be an ordinary floating window. On a Wayland session the
app therefore starts itself on XWayland (`GDK_BACKEND=x11`), where edge placement, keep-above,
no-focus and sticky-on-every-workspace all work. Set `CODENOTCH_NATIVE_WAYLAND=1` to opt out.

Two features see X11 windows only, and so quietly do nothing for Wayland-native terminals under a
Wayland session (they work fully on an Xorg session):

- **Seen-clears-it** — looking at the terminal of a finished session acknowledges it.
- **Jump back** — clicking a session in the card raises its terminal.

### Icons

Provider marks are the SVGs from [`@lobehub/icons-static-svg`](https://github.com/lobehub/lobe-icons)
(MIT), embedded unmodified — see `codenotch/glyphs/NOTICE.md`. Drop your own
`claude|codex|cursor|gemini.svg` (or `.png`) into `~/.config/codenotch/glyphs/` to override.
The marks remain the trademarks of their owners.

## Layout

```
.
├── codenotch/          Tauri 2 app: window, tray, providers (usage.rs, codex.rs, cursor.rs, antigravity.rs),
│   ├── src/            session engine (watcher.rs, state.rs, focus.rs), glyphs.rs, doctor.rs,
│   │                   linux.rs (X11 + /proc + xdg-open back-ends)
│   ├── ui/notch.html   the pill + hover card (single file, no framework)
│   └── glyphs/         provider marks (+ NOTICE.md)
├── codenotch-hook/     <5 ms hook messenger Claude Code calls; forwards events to the app
└── install.sh          user-level install: ~/.local/bin + a .desktop launcher
```

## Relationship to upstream

The Windows port follows the upstream design spec (`docs/specs/2026-08-28-usage-notch-design.md`)
and provider semantics; this fork keeps its Windows code paths intact so the two can stay in sync.
The session-detection engine originated in [Im-Midi/Pac-Man](https://github.com/Im-Midi/Pac-Man) (MIT).

## License

MIT — see `LICENSE`. The Codenotch design and name belong to the upstream author.
