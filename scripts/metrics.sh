set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
runs=${FLIT_RUNS:-3}
server=${FLIT_SERVER:-xvfb}
profile=${FLIT_PROFILE:-release}
data=${FLIT_DATA_DIR:-$target/testdata}
keys="start_to_first_frame_ms first_rows_ms full_listing_ms frame_cpu_p50_ms frame_cpu_p99_ms input_to_frame_p50_ms input_to_frame_max_ms dropped_frames sort_ms"

case $runs in
    ''|*[!0-9]*|0*) printf 'FLIT_RUNS must be a positive integer\n' >&2; exit 2 ;;
esac
case $server in
    xvfb|weston) ;;
    *) printf 'FLIT_SERVER must be xvfb or weston\n' >&2; exit 2 ;;
esac

cargo build --profile "$profile" -p flit -p flit-gen --manifest-path "$root/Cargo.toml"
[ -d "$data/flat_10k" ] && [ -d "$data/flat_100k" ] || "$target/$profile/flit-gen" "$data" --all >/dev/null
mkdir -p "$target/metrics"
out=$(mktemp -d "$target/metrics/$server-$profile-XXXXXX")
printf 'Measurements: %s\n' "$out"

sandbox=$(mktemp -d /tmp/flit-metrics-XXXXXX)
mkdir -p "$sandbox/config" "$sandbox/data" "$sandbox/state" "$sandbox/cache" "$sandbox/run"
chmod 700 "$sandbox/run"
export XDG_CONFIG_HOME=$sandbox/config XDG_DATA_HOME=$sandbox/data XDG_STATE_HOME=$sandbox/state
export XDG_CACHE_HOME=$sandbox/cache XDG_RUNTIME_DIR=$sandbox/run
unset WAYLAND_DISPLAY DISPLAY

case $server in
xvfb)
    display=:$((90 + RANDOM % 100))
    Xvfb "$display" -screen 0 1600x1000x24 -nolisten tcp >"$out/server.log" 2>&1 &
    export DISPLAY=$display
    ;;
weston)
    case $(weston --help 2>&1) in
        *--renderer*) weston_gl=(--backend=headless --renderer=gl) ;;
        *) weston_gl=(--backend=headless-backend.so --use-gl) ;;
    esac
    weston "${weston_gl[@]}" --socket=flit-metrics --width=1600 --height=1000 --idle-time=0 >"$out/server.log" 2>&1 &
    export WAYLAND_DISPLAY=flit-metrics
    ;;
esac
server_pid=$!
trap 'kill $server_pid 2>/dev/null; rm -rf "$sandbox"' EXIT
sleep 1
if ! kill -0 "$server_pid" 2>/dev/null; then
    printf 'Could not start %s. See %s/server.log\n' "$server" "$out" >&2
    exit 1
fi

median() {
    sort -g | awk '{v[NR]=$1} END {
        if (!NR) print "n/a"
        else if (NR % 2) print v[(NR+1)/2]
        else print (v[NR/2] + v[NR/2+1]) / 2
    }'
}

printf '| folder | %s |\n' "$(echo $keys | sed 's/ / | /g')"
for folder in flat_10k flat_100k; do
    for i in $(seq "$runs"); do
        sample=$out/$folder-$i
        if ! timeout 120 "$target/$profile/flit" --metrics "$sample.json" "$data/$folder" >"$sample.log" 2>&1 ||
            ! grep -q '"start_to_first_frame_ms":[0-9]' "$sample.json"; then
            printf 'No completed Flit measurement under %s: %s.log\n' "$server" "$sample" >&2
            exit 1
        fi
    done
    row="| $folder |"
    for k in $keys; do
        row="$row $(grep -hoE "\"$k\":[0-9]+(\.[0-9]+)?" "$out/$folder"-*.json | cut -d: -f2 | median) |"
    done
    echo "$row"
done
ls -l "$target/$profile/flit" | awk '{print "binary bytes:", $5}'
