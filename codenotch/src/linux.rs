//! Linux back-ends for what the Windows build does with Win32: the X server (pointer button state,
//! active window, window activation) via x11rb, the process table via /proc, and xdg-open for URLs
//! and folders. Every function degrades to "unknown" (None / 0 / false) instead of failing.
//!
//! Under a Wayland session the app runs on XWayland (main() switches the GDK backend), so X11
//! queries see X11 clients only: a Wayland-native terminal is invisible to the active-window and
//! jump-back helpers, and those features quietly do nothing for it. On an Xorg session they work
//! for every window.

use std::collections::HashMap;
use std::sync::OnceLock;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, KeyButMask, MapState, Window,
};
use x11rb::rust_connection::RustConnection;

// ---------------- xdg-open ----------------

/// Opens a URL or a folder with the desktop's default handler, detached from this process
pub fn open_external(target: &str) {
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = cmd.spawn();
}

// ---------------- /proc ----------------

pub struct ProcMaps {
    pub ppid: HashMap<u32, u32>,
    /// Lower-case process name (`comm`, 15 characters at most)
    pub name: HashMap<u32, String>,
}

/// One pass over /proc/<pid>/stat: `pid (comm) state ppid …`; comm may contain spaces and parentheses, so it is cut at the last `)`
pub fn proc_maps() -> ProcMaps {
    let mut m = ProcMaps {
        ppid: Default::default(),
        name: Default::default(),
    };
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return m;
    };
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
            continue;
        };
        let Some(open) = stat.find('(') else { continue };
        let Some(close) = stat.rfind(')') else {
            continue;
        };
        if close <= open {
            continue;
        }
        let comm = stat[open + 1..close].to_lowercase();
        // After ")": state ppid …
        let ppid = stat[close + 1..]
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        m.ppid.insert(pid, ppid);
        m.name.insert(pid, comm);
    }
    m
}

/// Command line with NUL separators turned into spaces (empty for kernel threads and other users' processes)
pub fn cmdline(pid: u32) -> String {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
        .unwrap_or_default()
}

/// (wchar, rchar) from /proc/<pid>/io: bytes written and read through any file or socket. Readable for our own processes only.
pub fn io_counters(pid: u32) -> Option<(u64, u64)> {
    let txt = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let mut rchar = None;
    let mut wchar = None;
    for line in txt.lines() {
        if let Some(v) = line.strip_prefix("rchar:") {
            rchar = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("wchar:") {
            wchar = v.trim().parse().ok();
        }
    }
    Some((wchar?, rchar?))
}

// ---------------- X11 ----------------

struct X11 {
    conn: RustConnection,
    root: Window,
    net_active_window: Atom,
    net_wm_pid: Atom,
    net_client_list: Atom,
    net_wm_state: Atom,
    net_wm_state_demands_attention: Atom,
}

static X11_CONN: OnceLock<Option<X11>> = OnceLock::new();

/// One connection for the life of the process (a fresh handshake every 8 ms during a drag is not free)
fn x11() -> Option<&'static X11> {
    X11_CONN
        .get_or_init(|| {
            let (conn, screen) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots.get(screen)?.root;
            let atom = |name: &[u8]| -> Option<Atom> {
                Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
            };
            let net_active_window = atom(b"_NET_ACTIVE_WINDOW")?;
            let net_wm_pid = atom(b"_NET_WM_PID")?;
            let net_client_list = atom(b"_NET_CLIENT_LIST")?;
            let net_wm_state = atom(b"_NET_WM_STATE")?;
            let net_wm_state_demands_attention = atom(b"_NET_WM_STATE_DEMANDS_ATTENTION")?;
            Some(X11 {
                conn,
                root,
                net_active_window,
                net_wm_pid,
                net_client_list,
                net_wm_state,
                net_wm_state_demands_attention,
            })
        })
        .as_ref()
}

/// Whether the left mouse button is held (drag loop); false when no X server is reachable
pub fn left_button_down() -> bool {
    let Some(x) = x11() else { return false };
    let Ok(cookie) = x.conn.query_pointer(x.root) else {
        return false;
    };
    let Ok(r) = cookie.reply() else { return false };
    u16::from(r.mask) & u16::from(KeyButMask::BUTTON1) != 0
}

fn window_prop(x: &X11, win: Window, prop: Atom, ty: AtomEnum, len: u32) -> Vec<u32> {
    x.conn
        .get_property(false, win, prop, ty, 0, len)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| r.value32().map(|it| it.collect()))
        .unwrap_or_default()
}

/// Pid of the process owning the active X window (0 when unknown, or when a Wayland-native window is active)
pub fn fg_pid() -> u32 {
    let Some(x) = x11() else { return 0 };
    let Some(win) = window_prop(x, x.root, x.net_active_window, AtomEnum::WINDOW, 1)
        .first()
        .copied()
    else {
        return 0;
    };
    if win == 0 {
        return 0;
    }
    window_prop(x, win, x.net_wm_pid, AtomEnum::CARDINAL, 1)
        .first()
        .copied()
        .unwrap_or(0)
}

/// (window, pid, area) for every managed, viewable X window with a pid, largest first
fn client_windows(x: &X11) -> Vec<(Window, u32, u64)> {
    let mut out = Vec::new();
    for win in window_prop(x, x.root, x.net_client_list, AtomEnum::WINDOW, u32::MAX) {
        let Some(pid) = window_prop(x, win, x.net_wm_pid, AtomEnum::CARDINAL, 1)
            .first()
            .copied()
        else {
            continue;
        };
        let viewable = x
            .conn
            .get_window_attributes(win)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|a| a.map_state == MapState::VIEWABLE)
            .unwrap_or(false);
        let area = x
            .conn
            .get_geometry(win)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|g| g.width as u64 * g.height as u64)
            .unwrap_or(0);
        // Minimised windows are not viewable but still worth raising; rank them below visible ones
        out.push((win, pid, if viewable { area } else { area / 4 }));
    }
    out.sort_by(|a, b| b.2.cmp(&a.2));
    out
}

/// Raise a window the way wmctrl does: an _NET_ACTIVE_WINDOW client message (source = pager), plus DEMANDS_ATTENTION so a focus-stealing refusal still flashes the taskbar entry
fn activate(x: &X11, win: Window) -> bool {
    let _ = x.conn.map_window(win);
    let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
    let ev = ClientMessageEvent::new(
        32,
        win,
        x.net_active_window,
        [2, x11rb::CURRENT_TIME, 0, 0, 0],
    );
    let ok = x.conn.send_event(false, x.root, mask, ev).is_ok();
    let attn = ClientMessageEvent::new(
        32,
        win,
        x.net_wm_state,
        [1, x.net_wm_state_demands_attention, 0, 2, 0],
    );
    let _ = x.conn.send_event(false, x.root, mask, attn);
    let _ = x.conn.flush();
    ok
}

/// Focus the largest window belonging to any of the given pids
pub fn focus_pids(pids: &[u32]) -> bool {
    if pids.is_empty() {
        return false;
    }
    let Some(x) = x11() else { return false };
    match client_windows(x)
        .into_iter()
        .find(|(_, pid, _)| pids.contains(pid))
    {
        Some((win, _, _)) => activate(x, win),
        None => false,
    }
}

/// Focus the largest window whose process name passes the filter
pub fn focus_named(filter: impl Fn(&str) -> bool) -> bool {
    let Some(x) = x11() else { return false };
    let maps = proc_maps();
    match client_windows(x)
        .into_iter()
        .find(|(_, pid, _)| maps.name.get(pid).map(|n| filter(n)).unwrap_or(false))
    {
        Some((win, _, _)) => activate(x, win),
        None => false,
    }
}
