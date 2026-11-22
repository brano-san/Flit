set -euo pipefail

if [[ $# -ne 1 || ! -x $1 ]]; then
    echo 'Usage: bash scripts/gui-a11y.sh path/to/flit' >&2
    exit 2
fi

binary=$(realpath -- "$1")
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
state=$(mktemp -d -t flit-gui.XXXXXXXX)
state=$(realpath -- "$state")
trap 'rm -rf -- "$state"' EXIT

mkdir -p -- "$state/home" "$state/config" "$state/cache" "$state/data" "$state/state" "$state/tmp"
mkdir -m 700 -- "$state/runtime"
export HOME="$state/home" XDG_CONFIG_HOME="$state/config" XDG_CACHE_HOME="$state/cache"
export XDG_DATA_HOME="$state/data" XDG_STATE_HOME="$state/state" XDG_RUNTIME_DIR="$state/runtime" TMPDIR="$state/tmp"
export DBUS_SYSTEM_BUS_ADDRESS="unix:path=$state/runtime/no-system-bus"
export GSETTINGS_BACKEND=memory LIBGL_ALWAYS_SOFTWARE=true LC_ALL=C.UTF-8
export FLIT_GUI_TEST=1 FLIT_NO_UPDATE=1 FLIT_LANG=en FLIT_LOG=1
unset WAYLAND_DISPLAY WAYLAND_SOCKET AT_SPI_BUS_ADDRESS NO_AT_BRIDGE FLIT_HIDDEN FLIT_HIDDEN_CAPTURE FLIT_RESTARTED_FROM
cd -- "$state"

xvfb-run -a -s '-screen 0 1280x900x24 -nolisten tcp' dbus-run-session -- \
    /usr/bin/python3 -I "$repo/crates/flit/test/gui_a11y.py" "$binary"
