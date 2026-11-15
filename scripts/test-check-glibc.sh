set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
binary=${1:?Usage: test-check-glibc.sh ELF_FILE}
test_root=$(mktemp -d)
trap 'rm -rf -- "$test_root"' EXIT
mkdir -p "$test_root/package/nested directory"
printf 'GLIBC_999.999\n' > "$test_root/package/plain text"
bash "$root/scripts/check-glibc.sh" 0.0 "$test_root/package"
cp -- "$binary" "$test_root/package/nested directory/elf file"
chmod 644 "$test_root/package/nested directory/elf file"
bash "$root/scripts/check-glibc.sh" 999.999 "$test_root/package"
if bash "$root/scripts/check-glibc.sh" 0.0 "$test_root/package" > "$test_root/output" 2>&1; then
    printf 'Expected incompatible ELF to be rejected\n' >&2
    exit 1
fi
grep -F "$test_root/package/nested directory/elf file requires GLIBC_" "$test_root/output"
grep -F 'maximum supported is GLIBC_0.0' "$test_root/output"
printf '\177ELFbroken' > "$test_root/broken"
mkdir "$test_root/invalid"
mv "$test_root/broken" "$test_root/invalid/elf"
if bash "$root/scripts/check-glibc.sh" 999.999 "$test_root/invalid" > "$test_root/output" 2>&1; then
    printf 'Expected unreadable ELF metadata to be rejected\n' >&2
    exit 1
fi
grep -F 'Cannot inspect GLIBC requirements:' "$test_root/output"
printf 'GLIBC checker tests passed.\n'
