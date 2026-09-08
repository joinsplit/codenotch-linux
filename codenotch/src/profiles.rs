//! Claude profiles. Claude Code keeps everything for one account under a single directory: `~/.claude`
//! by default, or wherever CLAUDE_CONFIG_DIR points. People who keep a personal and a work login apart
//! alias the second one to `~/.claude-work`, `~/.claude-client`, and so on — each with its own
//! credential, settings.json and projects folder. The upstream macOS app treats every such directory
//! as a profile with its own ring; this module mirrors that convention (the directories are the only
//! trace the profiles leave: the app is launched from a menu, so the alias's variable never reaches it).

use std::path::{Path, PathBuf};

pub const DEFAULT_ID: &str = "claude";
const DIR_PREFIX: &str = ".claude";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Profile {
    /// `claude` for `~/.claude`, `claude-<slug>` for `~/.claude-<slug>`; doubles as the provider id in the page
    pub id: String,
    /// `Claude`, or `Claude (work)`
    pub name: String,
    pub dir: PathBuf,
}

impl Profile {
    fn new(slug: Option<&str>, dir: PathBuf) -> Self {
        let (id, name) = match slug {
            Some(s) => (format!("{DEFAULT_ID}-{s}"), format!("Claude ({s})")),
            None => (DEFAULT_ID.to_string(), "Claude".to_string()),
        };
        Profile { id, name, dir }
    }

    /// The directory as a person would type it
    pub fn display_dir(&self) -> String {
        match dirs::home_dir().and_then(|h| self.dir.strip_prefix(h).ok().map(|p| p.to_path_buf())) {
            Some(rest) => format!("~/{}", rest.display()),
            None => self.dir.display().to_string(),
        }
    }
}

/// `.claude-work` → `work`; the bare `.claude` and the `.claude.json` file beside it are not slugs
fn slug_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(DIR_PREFIX)?.strip_prefix('-')?;
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

/// "Actually used": any of the files Claude Code creates the first time it runs against a directory.
/// An empty directory, or a stray one made by hand, would otherwise put a permanent "sign in" ring in the notch.
fn is_used(dir: &Path) -> bool {
    dir.is_dir()
        && [".credentials.json", "settings.json", "history.jsonl", "projects", "statsig", "sessions"]
            .iter()
            .any(|n| dir.join(n).exists())
}

/// The default profile followed by every `~/.claude-<slug>` Claude Code has used, slugs in alphabetical order so the rings never swap places between launches
pub fn discover() -> Vec<Profile> {
    let Some(home) = dirs::home_dir() else { return vec![Profile::new(None, PathBuf::from(".claude"))] };
    let mut out = vec![Profile::new(None, home.join(DIR_PREFIX))];
    let mut extras: Vec<(String, PathBuf)> = std::fs::read_dir(&home)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let slug = slug_of(&name)?.to_string();
                    let dir = e.path();
                    is_used(&dir).then_some((slug, dir))
                })
                .collect()
        })
        .unwrap_or_default();
    extras.sort();
    out.extend(extras.into_iter().map(|(slug, dir)| Profile::new(Some(&slug), dir)));
    out
}

/// The profile id a transcript belongs to, from its path: the first `.claude` / `.claude-<slug>` component.
/// Anything else (the desktop app's session mirrors, an unknown layout) is filed under the default profile.
pub fn id_for_path(p: &Path) -> String {
    for c in p.components() {
        let s = c.as_os_str().to_string_lossy();
        if s == DIR_PREFIX {
            return DEFAULT_ID.into();
        }
        if let Some(slug) = slug_of(&s) {
            return format!("{DEFAULT_ID}-{slug}");
        }
    }
    DEFAULT_ID.into()
}
