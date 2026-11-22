from pathlib import Path
import os
import subprocess
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib


ROW = (Atspi.Role.TABLE_ROW, Atspi.Role.LIST_ITEM)
PALETTE = "Select a command..."
COMMAND = "Toggle Hidden Files"


def children(node):
    for index in range(node.get_child_count()):
        child = node.get_child_at_index(index)
        if child is not None:
            yield child


def walk(node):
    yield node
    for child in children(node):
        yield from walk(child)


def find(node, name, roles):
    return next((item for item in walk(node) if item.get_role() in roles and item.get_name() == name), None)


def row(window, name):
    files = find(window, "Files", (Atspi.Role.TABLE, Atspi.Role.LIST))
    return find(files, name, ROW) if files is not None else None


def focused_entry(window, name, value):
    entry = find(window, name, (Atspi.Role.ENTRY,))
    if entry is not None and entry.get_state_set().contains(Atspi.StateType.FOCUSED):
        return Atspi.Text.get_text(entry, 0, -1) == value
    return False


def wait(app, description, probe):
    until = time.monotonic() + 20.0
    last_error = None
    while time.monotonic() < until:
        if app.poll() is not None:
            raise RuntimeError(f"Flit exited with status {app.returncode} while waiting for {description}")
        try:
            result = probe()
            if result:
                print(f"ok: {description}", flush=True)
                return result
        except GLib.Error as error:
            last_error = error
        time.sleep(0.1)
    raise RuntimeError(f"Timed out waiting for {description}") from last_error


def xdotool(*args):
    subprocess.run(["xdotool", *map(str, args)], check=True, timeout=5)


def click(node):
    bounds = node.get_extents(Atspi.CoordType.SCREEN)
    if bounds.width <= 0 or bounds.height <= 0:
        raise RuntimeError(f"Missing screen bounds for {node.get_name()}")
    time.sleep(0.6)
    xdotool("mousemove", bounds.x + bounds.width // 2, bounds.y + bounds.height // 2, "click", 1)


def scenario(app, folder):
    desktop = Atspi.get_desktop(0)

    def flit_window():
        desktop.clear_cache()
        for item in children(desktop):
            if item.get_name() == "Flit" and item.get_process_id() == app.pid:
                item.clear_cache()
                return next((child for child in children(item) if child.get_role() == Atspi.Role.FRAME), None)
        return None

    window = wait(app, "Flit window in the AT-SPI tree", flit_window)
    xids = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--pid", str(app.pid)], text=True, timeout=5).split()
    if len(xids) != 1:
        raise RuntimeError(f"Expected one visible Flit X11 window, found {xids}")
    xdotool("windowfocus", "--sync", xids[0])

    def check(description, probe):
        def fresh_tree():
            window.clear_cache()
            return probe(window)
        return wait(app, description, fresh_tree)

    folder_row = check("folder in the file list", lambda w: row(w, folder.name))
    click(folder_row)
    check("selected folder", lambda w: (item := row(w, folder.name)) and item.get_state_set().contains(Atspi.StateType.SELECTED))
    xdotool("key", "Return")

    original = folder / "draft.txt"
    hidden = folder / ".hidden.txt"
    file_row = check("file in the opened folder", lambda w: row(w, original.name))
    if row(window, hidden.name) is not None:
        raise AssertionError("The hidden file is visible before toggling hidden files")
    click(file_row)
    check("selected file", lambda w: (item := row(w, original.name)) and item.get_state_set().contains(Atspi.StateType.SELECTED))

    xdotool("key", "ctrl+shift+p")
    check("focused palette search", lambda w: focused_entry(w, PALETTE, ""))
    xdotool("type", "--clearmodifiers", COMMAND)
    check("filtered command result", lambda w: focused_entry(w, PALETTE, COMMAND) and find(w, COMMAND, (Atspi.Role.LIST_ITEM,)))
    xdotool("key", "Return")
    check("hidden file visible after executing the command", lambda w: row(w, hidden.name) and not find(w, PALETTE, (Atspi.Role.ENTRY,)))

    click(check("original file after refreshing the folder", lambda w: row(w, original.name)))
    xdotool("key", "F2")
    check("focused rename editor", lambda w: focused_entry(w, "Rename", original.name))
    xdotool("type", "ignored")
    check("edited name before cancellation", lambda w: focused_entry(w, "Rename", "ignored.txt"))
    xdotool("key", "Escape")
    check("rename cancelled in the tree", lambda w: row(w, original.name) and not find(w, "Rename", (Atspi.Role.ENTRY,)))
    if not original.is_file() or (folder / "ignored.txt").exists():
        raise AssertionError("Cancelling rename modified the fixture files")

    xdotool("key", "F2")
    check("rename editor reopened", lambda w: focused_entry(w, "Rename", original.name))
    xdotool("type", "renamed")
    new_name = folder / "renamed.txt"
    check("edited file name before commit", lambda w: focused_entry(w, "Rename", new_name.name))
    xdotool("key", "Return")
    check("renamed file and closed editor", lambda w: row(w, new_name.name) and not row(w, original.name) and not find(w, "Rename", (Atspi.Role.ENTRY,)))
    if original.exists() or new_name.read_text(encoding="utf-8") != "original contents\n":
        raise AssertionError("Renaming did not preserve the fixture contents under the new name")


def main(binary):
    if os.environ.get("FLIT_GUI_TEST") != "1" or os.environ.get("XDG_RUNTIME_DIR") != str(Path.cwd() / "runtime"):
        raise RuntimeError("Run this driver with scripts/gui-a11y.sh")
    root = Path.cwd() / "files"
    folder = root / "project"
    folder.mkdir(parents=True)
    (folder / "draft.txt").write_text("original contents\n", encoding="utf-8")
    (folder / ".hidden.txt").touch()
    subprocess.run([
        "gdbus", "call", "--session", "--dest", "org.a11y.Bus", "--object-path", "/org/a11y/bus",
        "--method", "org.freedesktop.DBus.Properties.Set", "org.a11y.Status", "IsEnabled", "<true>",
    ], check=True, timeout=10, stdout=subprocess.DEVNULL)
    Atspi.init()
    Atspi.set_timeout(1000, 1000)

    with open("flit.log", "w+", encoding="utf-8") as log:
        with subprocess.Popen([binary, str(root)], stdout=log, stderr=subprocess.STDOUT) as app:
            try:
                scenario(app, folder)
            except Exception:
                log.seek(0)
                print(log.read(), file=sys.stderr)
                raise
            finally:
                app.kill()


if __name__ == "__main__":
    main(sys.argv[1])
