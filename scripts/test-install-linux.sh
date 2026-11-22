#!/bin/sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
test_root=$(mktemp -d)
trap 'rm -rf -- "$test_root"' EXIT HUP INT TERM
stage=$test_root/package
export PREFIX="$test_root/install & space"
export XDG_CONFIG_HOME="$test_root/config"
fake=$test_root/fake-bin
export SYSTEMCTL_LOG="$test_root/systemctl.log"
export RESTORE_LOG="$test_root/restore.log"
mkdir -p "$fake"
printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> "$SYSTEMCTL_LOG"\n' > "$fake/systemctl"
chmod +x "$fake/systemctl"
export PATH="$fake:$PATH"
mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/icons/hicolor/16x16/apps" "$stage/share/doc/flit" "$stage/share/flit/dbus" "$stage/share/flit/systemd" "$stage/share/xdg-desktop-portal/portals"
cat > "$stage/bin/flit" <<'EOF'
#!/bin/sh
if [ "$*" = --restore-file-chooser ]; then
    printf '%s\n' "$*" >> "$RESTORE_LOG"
    exit "${RESTORE_STATUS:-0}"
fi
printf 'flit fixture\n'
EOF
cp "$root/assets/flit.desktop" "$stage/share/applications/"
cp "$root/assets/org.freedesktop.FileManager1.service" "$root/assets/org.freedesktop.impl.portal.desktop.flit.service" "$stage/share/flit/dbus/"
cp "$root/assets/flit.portal" "$stage/share/xdg-desktop-portal/portals/"
printf '#!/bin/sh\n' > "$stage/bin/flit-indexd"
tr -d '\r' < "$root/assets/flit-indexd.service" > "$stage/share/flit/systemd/flit-indexd.service"
tr -d '\r' < "$root/assets/flit-indexd.socket" > "$stage/share/flit/systemd/flit-indexd.socket"
cp "$root/LICENSE" "$stage/share/doc/flit/"
printf 'icon' > "$stage/share/icons/hicolor/16x16/apps/flit.png"
tr -d '\r' < "$root/scripts/install-linux.sh" > "$stage/install.sh"
sh -n "$stage/install.sh"
sh "$stage/install.sh"
test -x "$PREFIX/bin/flit"
test -f "$PREFIX/share/applications/flit.desktop"
test -f "$PREFIX/share/doc/flit/LICENSE"
test ! -f "$PREFIX/share/dbus-1/services/org.freedesktop.FileManager1.service"
portal="$PREFIX/share/xdg-desktop-portal/portals/flit.portal"
test -f "$portal"
grep -q '^Interfaces=org\.freedesktop\.impl\.portal\.FileChooser;' "$portal"
if grep -Eq '^[[:space:]]*UseIn[[:space:]]*=' "$portal"; then exit 1; fi
test ! -e "$XDG_CONFIG_HOME/xdg-desktop-portal/portals.conf"
test ! -e "$RESTORE_LOG"
grep -Fx "Exec=\"$PREFIX/bin/flit\" --file-chooser-portal" "$PREFIX/share/dbus-1/services/org.freedesktop.impl.portal.desktop.flit.service"
test -x "$PREFIX/bin/flit-indexd"
grep -Fx "ExecStart=\"$PREFIX/bin/flit-indexd\"" "$PREFIX/share/systemd/user/flit-indexd.service"
grep -Fx "WantedBy=default.target" "$PREFIX/share/systemd/user/flit-indexd.service"
test -f "$PREFIX/share/systemd/user/flit-indexd.socket"
if grep -q enable "$test_root/systemctl.log"; then exit 1; fi
sh "$stage/install.sh" --index-service
grep -Fx -e "--user enable --now flit-indexd.socket flit-indexd.service" "$test_root/systemctl.log"
sh "$stage/install.sh" --dbus-activation
grep -F "Exec=\"$PREFIX/bin/flit\" --dbus-activated" "$PREFIX/share/dbus-1/services/org.freedesktop.FileManager1.service"
printf 'keep' > "$PREFIX/share/doc/other.txt"
if RESTORE_STATUS=1 sh "$stage/install.sh" --uninstall; then exit 1; fi
test -x "$PREFIX/bin/flit"
test -f "$portal"
sh "$stage/install.sh" --uninstall
test "$(wc -l < "$RESTORE_LOG")" -eq 2
grep -Fx -e '--restore-file-chooser' "$RESTORE_LOG"
test ! -e "$PREFIX/bin/flit"
test ! -e "$PREFIX/share/doc/flit"
test ! -e "$PREFIX/share/dbus-1/services/org.freedesktop.FileManager1.service"
test ! -e "$PREFIX/share/xdg-desktop-portal/portals/flit.portal"
test ! -e "$PREFIX/share/dbus-1/services/org.freedesktop.impl.portal.desktop.flit.service"
test ! -e "$PREFIX/bin/flit-indexd"
test ! -e "$PREFIX/share/systemd/user/flit-indexd.service"
test ! -e "$PREFIX/share/systemd/user/flit-indexd.socket"
grep -Fx -e "--user disable --now flit-indexd.socket flit-indexd.service" "$test_root/systemctl.log"
test -f "$PREFIX/share/doc/other.txt"
mkdir -p "$PREFIX/bin"
cp -p "$stage/bin/flit" "$PREFIX/bin/flit"
RESTORE_STATUS=1 sh "$stage/install.sh" --uninstall
test ! -e "$PREFIX/bin/flit"
test "$(wc -l < "$RESTORE_LOG")" -eq 2
if sh "$stage/install.sh" --unknown; then exit 1; fi
if sh "$stage/install.sh" --uninstall --index-service; then exit 1; fi
echo 'Installer checks passed.'
