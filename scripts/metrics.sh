set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
runs=${FLIT_RUNS:-3}
server=${FLIT_SERVER:-xvfb}
data=$target/testdata
out=$target/metrics
keys="start_to_first_frame_ms first_rows_ms full_listing_ms frame_cpu_p50_ms frame_cpu_p99_ms input_to_frame_p50_ms input_to_frame_max_ms dropped_frames sort_ms"

cargo build --release -p flit -p flit-gen --manifest-path "$root/Cargo.toml"
[ -d "$data/flat_100k" ] || "$target/release/flit-gen" "$data" --all >/dev/null
mkdir -p "$out"

sandbox=$(mktemp -d /tmp/flit-metrics-XXXXXX)
mkdir -p "$sandbox/home" "$sandbox/run"
chmod 700 "$sandbox/run"
export HOME=$sandbox/home XDG_RUNTIME_DIR=$sandbox/run
unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME WAYLAND_DISPLAY DISPLAY

case $server in
xvfb)
    display=:$((90 + RANDOM % 100))
    Xvfb "$display" -screen 0 1600x1000x24 -nolisten tcp >/dev/null 2>&1 &
    export DISPLAY=$display
    ;;
weston)
    weston --backend=headless-backend.so --socket=flit-metrics --width=1600 --height=1000 --idle-time=0 >/dev/null 2>&1 &
    export WAYLAND_DISPLAY=flit-metrics
    ;;
*)
    echo "FLIT_SERVER must be xvfb or weston" >&2
    exit 2
    ;;
esac
server_pid=$!
trap 'kill $server_pid 2>/dev/null; rm -rf "$sandbox"' EXIT
sleep 1

median() {
    sort -g | awk '{v[NR]=$1} END {if (NR) print v[int((NR+1)/2)]; else print "n/a"}'
}

printf '| folder | %s |\n' "$(echo $keys | sed 's/ / | /g')"
for folder in flat_10k flat_100k; do
    for i in $(seq "$runs"); do
        rm -f "$out/$folder-$i.json"
        timeout 120 "$target/release/flit" --metrics "$out/$folder-$i.json" "$data/$folder" >/dev/null 2>&1 || true
    done
    row="| $folder |"
    for k in $keys; do
        row="$row $(cat "$out/$folder"-*.json 2>/dev/null | grep -o "\"$k\":[0-9.]*" | cut -d: -f2 | median) |"
    done
    echo "$row"
done
ls -l "$target/release/flit" | awk '{print "binary bytes:", $5}'
