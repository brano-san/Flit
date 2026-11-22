set -eu

max_glibc=2.35

root=$(cd "$(dirname "$0")/.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
version=$(tr -d '\r' < "$root/Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
arch=$(uname -m)
name=flit-$version-$arch-linux
out=$target/packages
stage=$out/$name

cargo build --profile dist -p flit -p flit-indexd --manifest-path "$root/Cargo.toml"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/icons" "$stage/share/doc/flit" "$stage/share/flit/dbus" "$stage/share/flit/systemd" "$stage/share/xdg-desktop-portal/portals"
install -m 755 "$target/dist/flit" "$stage/bin/flit"
install -m 755 "$target/dist/flit-indexd" "$stage/bin/flit-indexd"
cp "$root/assets/flit.desktop" "$stage/share/applications/"
cp "$root/assets/org.freedesktop.FileManager1.service" "$root/assets/org.freedesktop.impl.portal.desktop.flit.service" "$stage/share/flit/dbus/"
cp "$root/assets/flit.portal" "$stage/share/xdg-desktop-portal/portals/"
cp "$root/assets/flit-indexd.service" "$root/assets/flit-indexd.socket" "$stage/share/flit/systemd/"
cp -r "$root/assets/icon/hicolor" "$stage/share/icons/"
cp "$root/LICENSE" "$root/crates/flit-platform-linux-gui/assets/LICENSE-FlitSans.txt" "$root/crates/flit-platform-linux-gui/assets/LICENSE-FlitIcons.txt" "$stage/share/doc/flit/"
install -m 755 "$root/scripts/install-linux.sh" "$stage/install.sh"
bash "$root/scripts/check-glibc.sh" "$max_glibc" "$stage"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"

cd "$root"
if command -v cargo-deb >/dev/null; then
    cargo deb -p flit --no-build --no-strip --profile dist -o "$out/"
fi
if command -v cargo-generate-rpm >/dev/null; then
    cargo generate-rpm -p crates/flit --target-dir "$target" --profile dist -o "$out/"
fi
ls -l "$out"
