set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
arch=$(uname -m)
name=flit-$version-$arch-linux
out=$target/packages
stage=$out/$name

cargo build --profile dist -p flit --manifest-path "$root/Cargo.toml"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/icons" "$stage/share/doc/flit"
install -m 755 "$target/dist/flit" "$stage/bin/flit"
cp "$root/assets/flit.desktop" "$stage/share/applications/"
cp -r "$root/assets/icon/hicolor" "$stage/share/icons/"
cp "$root/LICENSE" "$root/crates/flit-platform-linux-gui/assets/LICENSE-FlitSans.txt" "$root/crates/flit-platform-linux-gui/assets/LICENSE-FlitIcons.txt" "$stage/share/doc/flit/"
install -m 755 "$root/scripts/install-linux.sh" "$stage/install.sh"
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
