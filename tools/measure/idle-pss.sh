#!/bin/sh
set -eu

case ${1-} in
    ''|*[!0-9]*|0)
        printf 'Usage: %s <Flit PID>\n' "$0" >&2
        exit 2
        ;;
esac

smaps=/proc/$1/smaps
if [ ! -r "$smaps" ]; then
    printf 'Cannot read %s\n' "$smaps" >&2
    exit 1
fi

LC_ALL=C awk '
    /^[[:xdigit:]]+-[[:xdigit:]]+[[:space:]]/ {
        path = $0
        sub(/^([^[:space:]]+[[:space:]]+){5}/, "", path)
        sub(/ \(deleted\)$/, "", path)
        lib = path
        sub(/^.*\//, "", lib)
        driver_dir = path ~ /\/(dri|gallium)\//
        driver_name = lib ~ /^(libnvidia-|lib(EGL|GLX|GLESv[12]|GL)_(mesa|nvidia)\.|libgallium|libvulkan_|(lib)?amdvlk)/
        driver = lib ~ /\.so(\.|$)/ && (driver_dir || driver_name)
    }
    $1 == "Pss:" {
        total += $2
        if (driver)
            gpu += $2
        samples++
    }
    END {
        if (!samples)
            exit 1
        printf "Process PSS: %.2f MiB\n", total / 1024
        printf "GPU driver library PSS: %.2f MiB\n", gpu / 1024
        printf "PSS excluding GPU driver libraries: %.2f MiB\n", (total - gpu) / 1024
    }
' "$smaps" || { printf 'No PSS sample from %s\n' "$smaps" >&2; exit 1; }
