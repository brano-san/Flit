#!/bin/sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
test_root=$(mktemp -d)
trap 'rm -rf -- "$test_root"' EXIT HUP INT TERM
stage=$test_root/package
export PREFIX="$test_root/install & space"
mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/icons/hicolor/16x16/apps" "$stage/share/doc/flit" "$stage/share/flit/dbus"
printf '#!/bin/sh\nprintf "flit fixture\\n"\n' > "$stage/bin/flit"
cp "$root/assets/flit.desktop" "$stage/share/applications/"
cp "$root/assets/org.freedesktop.FileManager1.service" "$stage/share/flit/dbus/"
cp "$root/LICENSE" "$stage/share/doc/flit/"
printf 'icon' > "$stage/share/icons/hicolor/16x16/apps/flit.png"
tr -d '\r' < "$root/scripts/install-linux.sh" > "$stage/install.sh"
sh -n "$stage/install.sh"
sh "$stage/install.sh"
test -x "$PREFIX/bin/flit"
test -f "$PREFIX/share/applications/flit.desktop"
test -f "$PREFIX/share/doc/flit/LICENSE"
test ! -f "$PREFIX/share/dbus-1/services/org.freedesktop.FileManager1.service"
sh "$stage/install.sh" --dbus-activation
grep -F "Exec=\"$PREFIX/bin/flit\" --dbus-activated" "$PREFIX/share/dbus-1/services/org.freedesktop.FileManager1.service"
printf 'keep' > "$PREFIX/share/doc/other.txt"
sh "$stage/install.sh" --uninstall
test ! -e "$PREFIX/bin/flit"
test ! -e "$PREFIX/share/doc/flit"
test ! -e "$PREFIX/share/dbus-1/services/org.freedesktop.FileManager1.service"
test -f "$PREFIX/share/doc/other.txt"
if sh "$stage/install.sh" --unknown; then exit 1; fi
echo 'Installer checks passed.'
