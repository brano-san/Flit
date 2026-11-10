#!/bin/sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
service=$prefix/share/dbus-1/services/org.freedesktop.FileManager1.service
case "${1:-}" in
    ""|--uninstall|--dbus-activation) ;;
    *) echo "Usage: $0 [--dbus-activation|--uninstall]" >&2; exit 2 ;;
esac

if [ "${1:-}" = "--uninstall" ]; then
    echo "Removing Flit from $prefix..."
    rm -f "$prefix/bin/flit" "$prefix/share/applications/flit.desktop"
    rm -rf "$prefix/share/doc/flit" "$prefix/share/flit"
    find "$prefix/share/icons/hicolor" -path '*/apps/flit.*' -delete 2>/dev/null || true
    if grep -Eqs 'flit"? --dbus-activated' "$service"; then
        echo "Removing Flit's D-Bus activation service..."
        rm -f "$service"
    fi
else
    echo "Installing Flit into $prefix..."
    mkdir -p "$prefix/bin" "$prefix/share"
    echo "Installing the executable..."
    install -m 755 "$here/bin/flit" "$prefix/bin/flit"
    echo "Installing application launcher, icons and documentation..."
    cp -r "$here/share/." "$prefix/share/"
    if [ "${1:-}" = "--dbus-activation" ]; then
        echo "Enabling D-Bus file manager activation..."
        mkdir -p "$(dirname "$service")"
        executable=$(printf '%s' "$prefix/bin/flit" | sed 's/[\\"]/\\&/g')
        printf '[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec="%s" --dbus-activated\n' "$executable" > "$service"
    fi
fi

echo "Refreshing desktop and icon caches..."
command -v update-desktop-database >/dev/null && update-desktop-database -q "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
echo "Done."
case "${1:-}:$PATH:" in
    --uninstall:*) ;;
    *":$prefix/bin:"*) ;;
    *) echo "Add $prefix/bin to PATH to start flit from a terminal." ;;
esac
