#!/bin/sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
service=$prefix/share/dbus-1/services/org.freedesktop.FileManager1.service
units=$prefix/share/systemd/user
dbus=no
index=no
uninstall=no
for arg in "$@"; do
    case "$arg" in
        --dbus-activation) dbus=yes ;;
        --index-service) index=yes ;;
        --uninstall) uninstall=yes ;;
        *) echo "Usage: $0 [--dbus-activation] [--index-service] | --uninstall" >&2; exit 2 ;;
    esac
done
if [ "$uninstall" = yes ] && [ "$dbus$index" != nono ]; then
    echo "Usage: $0 [--dbus-activation] [--index-service] | --uninstall" >&2
    exit 2
fi

if [ "$uninstall" = yes ]; then
    echo "Removing Flit from $prefix..."
    if grep -qsF flit-indexd "$units/flit-indexd.service"; then
        echo "Stopping and removing Flit's search index service..."
        if command -v systemctl >/dev/null; then
            systemctl --user disable --now flit-indexd.socket flit-indexd.service >/dev/null 2>&1 || true
        fi
        rm -f "$units/flit-indexd.service" "$units/flit-indexd.socket"
    fi
    rm -f "$prefix/bin/flit" "$prefix/bin/flit-indexd" "$prefix/share/applications/flit.desktop"
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
    if [ "$dbus" = yes ]; then
        echo "Enabling D-Bus file manager activation..."
        mkdir -p "$(dirname "$service")"
        executable=$(printf '%s' "$prefix/bin/flit" | sed 's/[\\"]/\\&/g')
        printf '[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec="%s" --dbus-activated\n' "$executable" > "$service"
    fi
    if [ -f "$here/bin/flit-indexd" ]; then
        echo "Installing the search index service..."
        install -m 755 "$here/bin/flit-indexd" "$prefix/bin/flit-indexd"
        mkdir -p "$units"
        executable=$(printf '%s' "$prefix/bin/flit-indexd" | sed 's/[\\"]/\\&/g; s/%/%%/g; s/\$/$$/g')
        while IFS= read -r line; do
            case "$line" in
                ExecStart=*) printf 'ExecStart="%s"\n' "$executable" ;;
                *) printf '%s\n' "$line" ;;
            esac
        done < "$here/share/flit/systemd/flit-indexd.service" > "$units/flit-indexd.service"
        cp "$here/share/flit/systemd/flit-indexd.socket" "$units/flit-indexd.socket"
        if ! command -v systemctl >/dev/null; then
            echo "systemctl is not available; Flit searches with its own index."
        elif [ "$index" = yes ]; then
            echo "Starting the search index service..."
            systemctl --user daemon-reload
            systemctl --user enable --now flit-indexd.socket flit-indexd.service
        else
            systemctl --user daemon-reload >/dev/null 2>&1 || true
            systemctl --user try-restart flit-indexd.service >/dev/null 2>&1 || true
            echo "To share one search index between all Flit windows, run: systemctl --user enable --now flit-indexd.socket flit-indexd.service"
        fi
    elif [ "$index" = yes ]; then
        echo "This package has no flit-indexd; Flit searches with its own index." >&2
    fi
fi

echo "Refreshing desktop and icon caches..."
command -v update-desktop-database >/dev/null && update-desktop-database -q "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
echo "Done."
case "$uninstall:$PATH:" in
    yes:*) ;;
    *":$prefix/bin:"*) ;;
    *) echo "Add $prefix/bin to PATH to start flit from a terminal." ;;
esac
