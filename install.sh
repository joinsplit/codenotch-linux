#!/usr/bin/env bash
# User-level install of a release build: both binaries into ~/.local/bin, an application launcher and icon.
# Run `cargo build --release` first. Re-run after rebuilding; a running instance must be quit from the tray first.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin="${XDG_DATA_HOME:-$HOME/.local/share}"
dest="$HOME/.local/bin"
for b in codenotch codenotch-hook; do
  [ -x "$here/target/release/$b" ] || { echo "missing target/release/$b — run: cargo build --release" >&2; exit 1; }
done
mkdir -p "$dest" "$bin/applications" "$bin/icons/hicolor/256x256/apps"
install -m 755 "$here/target/release/codenotch" "$here/target/release/codenotch-hook" "$dest/"
install -m 644 "$here/codenotch/icons/icon.png" "$bin/icons/hicolor/256x256/apps/codenotch.png"
cat > "$bin/applications/codenotch.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Codenotch
Comment=Usage notch for Claude, Codex, Cursor and Antigravity
Exec=$dest/codenotch
Icon=codenotch
Terminal=false
Categories=Utility;
StartupNotify=false
DESKTOP
command -v update-desktop-database >/dev/null && update-desktop-database "$bin/applications" 2>/dev/null || true
echo "installed to $dest (codenotch, codenotch-hook); launcher: $bin/applications/codenotch.desktop"
case ":$PATH:" in *":$dest:"*) ;; *) echo "note: $dest is not on your PATH" ;; esac
