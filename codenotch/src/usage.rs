//! Claude usage adapter (official), implemented from the upstream Codenotch's documented behaviour.
//! Endpoint: GET https://api.anthropic.com/api/oauth/usage
//! Headers: Authorization: Bearer <token>; anthropic-beta: oauth-2025-04-20; 15 s timeout
//! Rules (upstream's discipline):
//!   - the credential comes from Claude Code's own store (<profile dir>/.credentials.json), read only
//!   - one reading per Claude profile (~/.claude plus every used ~/.claude-<slug>, see profiles.rs), each with its own back-off
//!   - 401/403 → re-read the credential once and retry (Claude Code may have just refreshed the token) → still failing means needsAuth
//!   - 429 → back off 60 s × 2^n capped at 15 min, Retry-After only raises it; the deadline is persisted
//!   - never invent a percentage on failure: keep the last reading marked stale, and the UI shows how old it is
//! Reply (snake_case): { limits:[{kind,percent,resets_at}], five_hour:{utilization,resets_at}, seven_day:{...} }
//! limits is the forward-compatible main shape; five_hour/seven_day are merged in as a fallback (a window that just rolled over disappears from limits).

use crate::profiles::{self, Profile};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const POLL_ACTIVE_SECS: u64 = 60;
const POLL_IDLE_SECS: u64 = 300;
const BACKOFF_BASE_SECS: u64 = 60;
const BACKOFF_CAP_SECS: u64 = 900;

static REFRESH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Immediate refresh from the tray or a command
pub fn request_refresh() {
    REFRESH.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Sleep in slices so request_refresh can interrupt it
fn sleep_interruptible(total_secs: u64) {
    for _ in 0..total_secs {
        if REFRESH.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// What the page receives: every profile with its reading, in display order
#[derive(Debug, Clone, Serialize)]
pub struct ProfileUsage {
    pub id: String,
    pub name: String,
    pub dir: String,
    pub snap: UsageSnapshot,
}

pub fn profile_usages(app: &AppHandle) -> Vec<ProfileUsage> {
    let st = app.state::<AppState>();
    let profiles = st.profiles.lock().unwrap().clone();
    let usage = st.usage.lock().unwrap();
    profiles
        .iter()
        .map(|p| ProfileUsage {
            id: p.id.clone(),
            name: p.name.clone(),
            dir: p.display_dir(),
            snap: usage.get(&p.id).cloned().unwrap_or_default(),
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LimitWindow {
    pub id: String,
    pub label: String,
    /// 0.0–1.0 (fraction used)
    pub used: f64,
    /// Reset time, ms epoch (None = unknown)
    pub resets_at: Option<u64>,
    /// Pure count window (no published denominator, e.g. Antigravity's requests today) — the cell shows ~N and the ring draws only its track
    #[serde(default)]
    pub count: Option<i64>,
    /// The number is ours, not the vendor's (upstream fidelity=.derived) — the card adds a ~ prefix
    #[serde(default)]
    pub derived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageSnapshot {
    /// ok | stale | needsAuth | backoff | error
    pub status: String,
    pub windows: Vec<LimitWindow>,
    pub fetched_at: u64,
    pub note: String,
    #[serde(default)]
    pub backoff_until: u64,
}

/// usage.json for the default profile (unchanged from before profiles existed), usage-<slug>.json for the others
fn store_path(id: &str) -> std::path::PathBuf {
    let name = match id.strip_prefix(&format!("{}-", profiles::DEFAULT_ID)) {
        Some(slug) => format!("usage-{slug}.json"),
        None => "usage.json".into(),
    };
    crate::config::config_path().with_file_name(name)
}

fn load_one(id: &str) -> UsageSnapshot {
    std::fs::read_to_string(store_path(id))
        .ok()
        .and_then(|t| serde_json::from_str::<UsageSnapshot>(&t).ok())
        .map(|mut s| {
            if !s.windows.is_empty() {
                s.status = "stale".into(); // an old reading after a restart is labelled as such
            }
            s
        })
        .unwrap_or_default()
}

pub fn load_persisted(profiles: &[Profile]) -> HashMap<String, UsageSnapshot> {
    profiles.iter().map(|p| (p.id.clone(), load_one(&p.id))).collect()
}

fn persist(id: &str, s: &UsageSnapshot) {
    if let Ok(t) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(store_path(id), t);
    }
}

/// Reads Claude Code's OAuth credential for one profile directory. Returns (token, expired hint).
fn read_credentials(dir: &Path) -> Option<(String, bool)> {
    for name in [".credentials.json", "credentials.json"] {
        let p = dir.join(name);
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let oauth = v.get("claudeAiOauth").unwrap_or(&v);
        if let Some(tok) = oauth.get("accessToken").and_then(|x| x.as_str()) {
            let expired = oauth
                .get("expiresAt")
                .and_then(|x| x.as_f64())
                .map(|ms| (ms as u64) <= now_ms())
                .unwrap_or(false);
            return Some((tok.to_string(), expired));
        }
    }
    None
}

/// For doctor: one credential probe line per profile (prints no secret values)
pub fn probe_credentials() -> String {
    profiles::discover()
        .iter()
        .map(|p| match read_credentials(&p.dir) {
            Some((tok, expired)) => format!(
                "credential {} ({}): found (token {} chars, {})",
                p.name,
                p.display_dir(),
                tok.len(),
                if expired { "expired — Claude Code refreshes it on its next use" } else { "valid" }
            ),
            None => format!(
                "credential {} ({}): .credentials.json not found (needsAuth; signing in once with the Claude Code CLI against that directory creates it)",
                p.name,
                p.display_dir()
            ),
        })
        .collect::<Vec<_>>()
        .join("\n  ")
}

fn parse_reset(v: &serde_json::Value) -> Option<u64> {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis().max(0) as u64)
}

fn label_for(kind: &str) -> String {
    match kind {
        "session" => "Current session".into(),
        "seven_day" | "weekly_all" => "Weekly (all models)".into(),
        "seven_day_opus" | "weekly_opus" => "Weekly (Opus)".into(),
        "weekly_scoped" => "Weekly (model-scoped)".into(),
        other => {
            // Forward compatibility: an unknown kind gets a readable label
            let mut s = other.replace('_', " ");
            if let Some(c) = s.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            s
        }
    }
}

fn parse_response(v: &serde_json::Value) -> Vec<LimitWindow> {
    let mut out: Vec<LimitWindow> = Vec::new();
    if let Some(arr) = v.get("limits").and_then(|x| x.as_array()) {
        for l in arr {
            let Some(kind) = l.get("kind").and_then(|x| x.as_str()) else {
                continue;
            };
            let Some(pct) = l.get("percent").and_then(|x| x.as_f64()) else {
                continue;
            };
            let resets = l.get("resets_at").and_then(parse_reset);
            if resets.is_none() {
                continue; // upstream rule: a window without a reset time is not shown
            }
            out.push(LimitWindow {
                id: kind.to_string(),
                label: label_for(kind),
                used: (pct / 100.0).clamp(0.0, 1.0),
                resets_at: resets, ..Default::default()
            });
        }
    }
    // Fallback merge: a window that just rolled over disappears from limits while the named field remains.
    // In practice the kinds in limits are weekly_all/weekly_scoped, not seven_day — deduplicating by id
    // alone would add the seven_day fallback a second time (the card showed "Weekly all" and
    // "Weekly (all models)" as twins). Three dedupe rules: id alias / same resets_at and percentage / same label.
    let aliases: [(&str, &str, &[&str]); 2] = [
        ("five_hour", "session", &["session", "five_hour"]),
        ("seven_day", "seven_day", &["seven_day", "weekly_all", "weekly"]),
    ];
    for (field, id, alias) in aliases {
        let Some(w) = v.get(field) else { continue };
        let Some(u) = w.get("utilization").and_then(|x| x.as_f64()) else { continue };
        let used = (u / 100.0).clamp(0.0, 1.0);
        let resets_at = w.get("resets_at").and_then(parse_reset);
        let label = label_for(id);
        let dup = out.iter().any(|x| {
            alias.contains(&x.id.as_str())
                || x.label == label
                || (resets_at.is_some()
                    && x.resets_at.map(|r| r / 1000) == resets_at.map(|r| r / 1000)
                    && (x.used - used).abs() < 0.005)
        });
        if dup {
            continue;
        }
        out.push(LimitWindow { id: id.into(), label, used, resets_at, ..Default::default() });
    }
    // session always comes first (upstream display order)
    out.sort_by_key(|w| if w.id == "session" { 0 } else { 1 });
    out
}

enum FetchErr {
    NeedsAuth,
    RateLimited(u64), // suggested wait in seconds (the Retry-After before the floor is applied)
    Other(String),
}

fn fetch_once(token: &str) -> Result<Vec<LimitWindow>, FetchErr> {
    let resp = ureq::get(ENDPOINT)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .timeout(Duration::from_secs(15))
        .call();
    match resp {
        Ok(r) => {
            let v: serde_json::Value = r
                .into_json()
                .map_err(|e| FetchErr::Other(format!("parse: {e}")))?;
            Ok(parse_response(&v))
        }
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => {
            Err(FetchErr::NeedsAuth)
        }
        Err(ureq::Error::Status(429, r)) => {
            let ra = r
                .header("retry-after")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            Err(FetchErr::RateLimited(ra))
        }
        Err(ureq::Error::Status(code, _)) => Err(FetchErr::Other(format!("HTTP {code}"))),
        Err(e) => Err(FetchErr::Other(format!("{e}"))),
    }
}

fn backoff_secs(consecutive: u32, retry_after_floor: u64) -> u64 {
    let exp = BACKOFF_BASE_SECS.saturating_mul(1u64 << consecutive.min(4));
    exp.clamp(BACKOFF_BASE_SECS, BACKOFF_CAP_SECS).max(retry_after_floor)
}

fn set_and_broadcast(app: &AppHandle, id: &str, mutate: impl FnOnce(&mut UsageSnapshot)) {
    let snap = {
        let st = app.state::<AppState>();
        let mut all = st.usage.lock().unwrap();
        let u = all.entry(id.to_string()).or_default();
        mutate(u);
        u.clone()
    };
    persist(id, &snap);
    broadcast(app);
}

/// The page always gets the whole list, so a new profile or a changed order needs no special event
pub fn broadcast(app: &AppHandle) {
    let _ = app.emit("usage", profile_usages(app));
}

/// Profiles appear when Claude Code is first run against a new directory: pick them up on every cycle (a read_dir of $HOME)
fn refresh_profiles(app: &AppHandle) -> Vec<Profile> {
    let found = profiles::discover();
    let st = app.state::<AppState>();
    let mut cur = st.profiles.lock().unwrap();
    if *cur != found {
        crate::applog(&format!("claude profiles: {}", found.iter().map(|p| p.display_dir()).collect::<Vec<_>>().join(", ")));
        let mut usage = st.usage.lock().unwrap();
        for p in &found {
            usage.entry(p.id.clone()).or_insert_with(|| load_one(&p.id));
        }
        *cur = found.clone();
    }
    found
}

/// One poll of one profile; returns the number of consecutive 429s to carry forward
fn poll_profile(app: &AppHandle, p: &Profile, consecutive_429: u32) -> u32 {
    let id = p.id.as_str();
    match read_credentials(&p.dir) {
        None => {
            set_and_broadcast(app, id, |u| {
                u.status = "needsAuth".into();
                u.note = format!("No Claude Code credential in {}", p.display_dir());
            });
            consecutive_429
        }
        Some((token, expired)) => {
            // On 401/403 re-read the credential and retry once (Claude Code may have just refreshed it)
            let result = match fetch_once(&token) {
                Err(FetchErr::NeedsAuth) => match read_credentials(&p.dir) {
                    Some((t2, _)) if t2 != token => fetch_once(&t2),
                    _ => Err(FetchErr::NeedsAuth),
                },
                other => other,
            };
            let auth_note = if expired {
                "Credential expired — run any claude command (or chat with Claude) to refresh it"
            } else {
                "Credential rejected (switched accounts?)"
            };
            match result {
                Ok(windows) => {
                    set_and_broadcast(app, id, |u| {
                        u.status = "ok".into();
                        u.windows = windows;
                        u.fetched_at = now_ms();
                        u.note.clear();
                        u.backoff_until = 0;
                    });
                    0
                }
                Err(FetchErr::NeedsAuth) => {
                    set_and_broadcast(app, id, |u| {
                        u.status = "needsAuth".into();
                        u.note = auth_note.into();
                    });
                    consecutive_429
                }
                Err(FetchErr::RateLimited(ra)) => {
                    let wait = backoff_secs(consecutive_429, ra);
                    set_and_broadcast(app, id, |u| {
                        if !u.windows.is_empty() {
                            u.status = "stale".into();
                        }
                        u.note = format!("Rate limited, retrying in {wait}s");
                        u.backoff_until = now_ms() + wait * 1000;
                    });
                    consecutive_429 + 1
                }
                Err(FetchErr::Other(msg)) => {
                    set_and_broadcast(app, id, |u| {
                        if u.windows.is_empty() {
                            u.status = "error".into();
                        } else {
                            u.status = "stale".into();
                        }
                        u.note = msg;
                    });
                    consecutive_429
                }
            }
        }
    }
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        // Broadcast the persisted old readings at startup (stale beats blank)
        broadcast(&app);
        let mut consecutive_429: HashMap<String, u32> = HashMap::new();
        loop {
            let profiles = refresh_profiles(&app);
            let now = now_ms();
            let mut earliest_retry: Option<u64> = None;
            for p in &profiles {
                // No requests inside a profile's backoff window
                let bu = {
                    let st = app.state::<AppState>();
                    let u = st.usage.lock().unwrap();
                    u.get(&p.id).map(|s| s.backoff_until).unwrap_or(0)
                };
                if bu > now {
                    earliest_retry = Some(earliest_retry.map_or(bu, |e: u64| e.min(bu)));
                    continue;
                }
                let n = consecutive_429.get(&p.id).copied().unwrap_or(0);
                let n = poll_profile(&app, p, n);
                consecutive_429.insert(p.id.clone(), n);
            }
            // 60 s while a session is active, 300 s otherwise (upstream throttling discipline); sooner if a back-off ends first
            let active = {
                let st = app.state::<AppState>();
                let store = st.store.lock().unwrap();
                let s = store.snapshot("en", "en", false);
                !s.sessions.is_empty()
            };
            let mut wait = if active { POLL_ACTIVE_SECS } else { POLL_IDLE_SECS };
            if let Some(bu) = earliest_retry {
                wait = wait.min(((bu.saturating_sub(now_ms())) / 1000).clamp(1, 30));
            }
            sleep_interruptible(wait);
        }
    });
}
