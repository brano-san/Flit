set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$prefix/bin/flit" "$prefix/share/applications/flit.desktop"
    rm -rf "$prefix/share/doc/flit"
    find "$prefix/share/icons/hicolor" -path '*/apps/flit.*' -delete 2>/dev/null || true
else
    mkdir -p "$prefix/bin" "$prefix/share"
    install -m 755 "$here/bin/flit" "$prefix/bin/flit"
    cp -r "$here/share/." "$prefix/share/"
fi

command -v update-desktop-database >/dev/null && update-desktop-database -q "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "Add $prefix/bin to PATH to start flit from a terminal." ;;
esac
