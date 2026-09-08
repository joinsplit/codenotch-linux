//! Start at sign-in. Windows: an HKCU\...\Run registry value (per user, no administrator needed),
//! written with reg.exe so there is no new dependency. Linux and other freedesktop systems: an XDG
//! autostart entry, ~/.config/autostart/codenotch.desktop. The command carries --silent: wait in
//! the background, show no bar without sessions, appear when one starts.

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;

    fn entry_path() -> Option<PathBuf> {
        dirs::config_dir().map(|c| c.join("autostart").join("codenotch.desktop"))
    }

    pub fn is_enabled() -> bool {
        entry_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| !t.lines().any(|l| l.trim() == "Hidden=true"))
            .unwrap_or(false)
    }

    pub fn enable() -> Result<String, String> {
        let path = entry_path().ok_or("cannot find the config directory")?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let body = format!(
            "[Desktop Entry]\nType=Application\nName=Codenotch\nComment=Usage notch for Claude, Codex, Cursor and Antigravity\nExec=\"{}\" --silent\nIcon=codenotch\nTerminal=false\nX-GNOME-Autostart-enabled=true\nStartupNotify=false\n",
            exe.display()
        );
        std::fs::write(&path, body).map_err(|e| e.to_string())?;
        Ok(format!("start at sign-in enabled ({})", path.display()))
    }

    pub fn disable() -> Result<String, String> {
        let path = entry_path().ok_or("cannot find the config directory")?;
        if !path.exists() {
            return Ok("start at sign-in was not enabled".into());
        }
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        Ok("start at sign-in disabled".into())
    }
}

#[cfg(windows)]
mod imp {
    use std::process::Command;

    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "Codenotch";

    fn reg(args: &[&str]) -> Option<(bool, String)> {
        let mut c = Command::new("reg");
        c.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        c.output().ok().map(|o| {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            (o.status.success(), text)
        })
    }

    pub fn is_enabled() -> bool {
        reg(&["query", RUN_KEY, "/v", NAME])
            .map(|(ok, out)| ok && out.contains(NAME))
            .unwrap_or(false)
    }

    pub fn enable() -> Result<String, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let val = format!("\"{}\" --silent", exe.display());
        match reg(&["add", RUN_KEY, "/v", NAME, "/t", "REG_SZ", "/d", &val, "/f"]) {
            Some((true, _)) => {
                Ok("start at sign-in enabled (silent until a session appears)".into())
            }
            Some((false, out)) => Err(out),
            None => Err("reg.exe failed to run".into()),
        }
    }

    pub fn disable() -> Result<String, String> {
        match reg(&["delete", RUN_KEY, "/v", NAME, "/f"]) {
            Some((true, _)) => Ok("start at sign-in disabled".into()),
            Some((false, out)) => {
                if out.to_lowercase().contains("unable to find") || out.contains("找不到") {
                    // reg.exe answers in the OS language; "找不到" is the Chinese "unable to find"
                    Ok("start at sign-in was not enabled".into())
                } else {
                    Err(out)
                }
            }
            None => Err("reg.exe failed to run".into()),
        }
    }
}

pub use imp::{disable, enable, is_enabled};
