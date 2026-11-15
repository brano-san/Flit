set -euo pipefail

maximum=${1:?Usage: check-glibc.sh MAXIMUM DIRECTORY}
directory=${2:?Usage: check-glibc.sh MAXIMUM DIRECTORY}
files=$(mktemp)
trap 'rm -f "$files"' EXIT
find "$directory" -type f -print0 > "$files"
while IFS= read -r -d '' file; do
    magic=$(od -An -N4 -tx1 -- "$file" | tr -d ' \n')
    if [ "$magic" != 7f454c46 ]; then
        continue
    fi
    if ! symbols=$(LC_ALL=C readelf --version-info -- "$file"); then
        printf 'Cannot inspect GLIBC requirements: %s\n' "$file" >&2
        exit 1
    fi
    required=$(printf '%s\n' "$symbols" | sed -n 's/.*Name: GLIBC_\([0-9][0-9.]*\).*/\1/p' | sort -Vu | tail -n 1)
    if [ -n "$required" ] && [ "$(printf '%s\n' "$maximum" "$required" | sort -V | tail -n 1)" != "$maximum" ]; then
        printf 'GLIBC compatibility check failed: %s requires GLIBC_%s; maximum supported is GLIBC_%s\n' "$file" "$required" "$maximum" >&2
        exit 1
    fi
    printf 'GLIBC compatibility check passed: %s (required: %s, maximum: %s)\n' "$file" "${required:-none}" "$maximum"
done < "$files"
