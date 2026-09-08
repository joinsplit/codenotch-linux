//! Merges codenotch-hook into ~/.claude/settings.json without overwriting the user's own hooks.
//! Identification: the command contains "codenotch-hook". A backup is written first.

use serde_json::{json, Value};
use std::path::PathBuf;

/// (Claude Code event name, whether it needs a matcher, the internal event reported to Codenotch)
const WIRING: &[(&str, bool, &str)] = &[
    ("SessionStart", false, "session_start"),
    ("UserPromptSubmit", false, "running"),
    ("PreToolUse", true, "running"),
    ("PostToolUse", true, "running"),
    ("Notification", false, "attention"),
    ("Stop", false, "done"),
    ("SessionEnd", false, "session_end"),
];

/// One settings.json per Claude profile (~/.claude, ~/.claude-work, …): a hook installed in one alone would leave the other profile's sessions invisible
fn settings_paths() -> Vec<PathBuf> {
    crate::profiles::discover().into_iter().map(|p| p.dir.join("settings.json")).collect()
}

fn is_ours(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .map(|hs| {
            hs.iter().any(|h| {
                h["command"]
                    .as_str()
                    .map(|c| c.contains("codenotch-hook") || c.contains("eatbean-hook") || c.contains("pacman-hook"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn load(path: &PathBuf) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

fn backup_and_write(path: &PathBuf, root: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = std::fs::copy(path, path.with_extension(format!("json.codenotch-bak-{ts}")));
    }
    let txt = serde_json::to_string_pretty(root).map_err(|e| e.to_string())?;
    std::fs::write(path, txt).map_err(|e| e.to_string())
}

#[allow(dead_code)]
pub fn is_installed() -> bool {
    settings_paths()
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .any(|t| t.contains("codenotch-hook"))
}

pub fn install() -> Result<String, String> {
    let paths = settings_paths();
    if paths.is_empty() {
        return Err("cannot find the user directory".into());
    }
    let mut done = Vec::new();
    for path in &paths {
        install_into(path)?;
        done.push(path.display().to_string());
    }
    Ok(format!("wrote {} ({} events each)", done.join(", "), WIRING.len()))
}

fn install_into(path: &PathBuf) -> Result<(), String> {
    let hook_exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("cannot locate the program directory")?
        .join(format!("codenotch-hook{}", std::env::consts::EXE_SUFFIX));
    if !hook_exe.exists() {
        return Err(format!("missing {}", hook_exe.display()));
    }

    let mut root = load(path);
    if !root.is_object() {
        root = json!({});
    }
    if !root["hooks"].is_object() {
        root["hooks"] = json!({});
    }

    for (event, need_matcher, internal) in WIRING {
        let arr = root["hooks"][*event].as_array().cloned().unwrap_or_default();
        // Remove our own older entries first
        let mut arr: Vec<Value> = arr.into_iter().filter(|e| !is_ours(e)).collect();
        let cmd = format!("\"{}\" {}", hook_exe.display(), internal);
        let mut entry = json!({
            "hooks": [{ "type": "command", "command": cmd, "timeout": 5 }]
        });
        if *need_matcher {
            entry["matcher"] = json!("*");
        }
        arr.push(entry);
        root["hooks"][*event] = json!(arr);
    }

    backup_and_write(path, &root)
}

pub fn uninstall() -> Result<String, String> {
    let mut removed = 0;
    let mut touched = 0;
    for path in settings_paths() {
        if !path.exists() {
            continue;
        }
        removed += uninstall_from(&path)?;
        touched += 1;
    }
    if touched == 0 {
        return Ok("settings.json does not exist, nothing to uninstall".into());
    }
    Ok(format!("removed {removed} Codenotch hook(s) from {touched} settings file(s)"))
}

fn uninstall_from(path: &PathBuf) -> Result<usize, String> {
    let mut root = load(path);
    let Some(hooks) = root["hooks"].as_object_mut() else {
        return Ok(0);
    };
    let mut removed = 0;
    for (_, v) in hooks.iter_mut() {
        if let Some(arr) = v.as_array() {
            let filtered: Vec<Value> = arr.iter().filter(|e| !is_ours(e)).cloned().collect();
            removed += arr.len() - filtered.len();
            *v = json!(filtered);
        }
    }
    if removed > 0 {
        backup_and_write(path, &root)?;
    }
    Ok(removed)
}
