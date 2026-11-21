#!/bin/sh
# Install IRCord for the current user: the binary in ~/.local/bin, the desktop
# entry and icon under ~/.local/share. `./install.sh --uninstall` removes them.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
apps="$data/applications"
icons="$data/icons/hicolor"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin/ircord" "$apps/com.ircord.gpui.desktop" \
        "$icons/scalable/apps/com.ircord.gpui.svg" "$icons/256x256/apps/com.ircord.gpui.png"
    echo "IRCord removed. Its settings stay in ${XDG_CONFIG_HOME:-$HOME/.config}/com.ircord.gpui."
    exit 0
fi

mkdir -p "$bin" "$apps" "$icons/scalable/apps" "$icons/256x256/apps"
install -m 755 "$here/ircord" "$bin/ircord"
install -m 644 "$here/ircord.svg" "$icons/scalable/apps/com.ircord.gpui.svg"
install -m 644 "$here/ircord.png" "$icons/256x256/apps/com.ircord.gpui.png"
sed "s|^Exec=ircord|Exec=$bin/ircord|" "$here/com.ircord.gpui.desktop" >"$apps/com.ircord.gpui.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$icons" 2>/dev/null || true
echo "IRCord installed: $bin/ircord, with a menu entry."
