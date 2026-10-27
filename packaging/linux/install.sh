#!/bin/sh
# Simply3D for Linux: installs for the current user, no root needed.
#   ./install.sh              add the simply3d command, a menu entry and the icon
#   ./install.sh --uninstall  remove them again
set -e
here="$(cd "$(dirname "$0")" && pwd)"
bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
apps="$data/applications"
icons="$data/icons/hicolor/256x256/apps"

if [ "$1" = "--uninstall" ]; then
    rm -f "$bin/simply3d" "$apps/simply3d.desktop" "$icons/simply3d.png" "$data/icons/hicolor/scalable/apps/simply3d.svg"
    update-desktop-database "$apps" 2>/dev/null || true
    echo "Simply3D removed."
    exit 0
fi

mkdir -p "$bin" "$apps" "$icons"
install -m 755 "$here/simply3d" "$bin/simply3d"
install -m 644 "$here/simply3d.png" "$icons/simply3d.png"
# the icon of older versions
rm -f "$data/icons/hicolor/scalable/apps/simply3d.svg"
sed "s|^Exec=.*|Exec=\"$bin/simply3d\" %F|" "$here/simply3d.desktop" > "$apps/simply3d.desktop"
chmod 644 "$apps/simply3d.desktop"
update-desktop-database "$apps" 2>/dev/null || true
gtk-update-icon-cache -q "$data/icons/hicolor" 2>/dev/null || true

echo "Simply3D installed: find it in your applications menu, or run $bin/simply3d"
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "(Add $bin to your PATH to start it by typing simply3d.)" ;;
esac
