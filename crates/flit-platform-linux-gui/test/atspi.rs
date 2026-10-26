use super::*;
use flit_core::render::Rect;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const WIN: WindowId = 7;
const BACK: u64 = 0x10;
const ALPHA: u64 = 0x21;
const BETA: u64 = 0x22;

fn sample(focus_beta: bool, beta_name: &str) -> Tree {
    let mut t = Tree::new("Flit test", Rect::new(0.0, 0.0, 800.0, 600.0));
    let tabs = t.add(0, 2, Role::TabList, "Tabs", Rect::new(0.0, 0.0, 800.0, 30.0));
    let tab = t.add(tabs, 0x11, Role::Tab, "Home", Rect::new(0.0, 0.0, 120.0, 30.0));
    t.nodes[tab].selected = true;
    let bar = t.add(0, 3, Role::ToolBar, "Navigation", Rect::new(0.0, 30.0, 800.0, 30.0));
    t.add(bar, BACK, Role::Button, "Back", Rect::new(0.0, 30.0, 30.0, 30.0));
    let edit = t.add(0, 4, Role::Edit, "Address", Rect::new(40.0, 30.0, 400.0, 30.0));
    t.nodes[edit].value = "/tmp/flit".into();
    let list = t.add(0, 7, Role::List, "Files", Rect::new(0.0, 60.0, 800.0, 540.0));
    let a = t.add(list, ALPHA, Role::ListItem, "alpha.txt", Rect::new(0.0, 60.0, 800.0, 24.0));
    t.nodes[a].selected = true;
    let b = t.add(list, BETA, Role::ListItem, beta_name, Rect::new(0.0, 84.0, 800.0, 24.0));
    t.focus = Some(if focus_beta { b } else { list });
    t
}

#[test]
fn object_paths_round_trip() {
    for r in [Ref::App, Ref::Node(3, ROOT_ID), Ref::Node(u64::MAX, u64::MAX)] {
        assert_eq!(Ref::parse(&r.path()), Some(r));
    }
    assert_eq!(Ref::parse(NULL_PATH), None);
    assert_eq!(Ref::parse("/org/a11y/atspi/accessible/wxnq"), None);
}

#[test]
fn changes_report_focus_names_selection_and_children() {
    let old = sample(false, "beta");
    let mut new = sample(true, "beta2");
    let (alpha, beta, gone, bar) = (new.find(ALPHA).unwrap(), new.find(BETA).unwrap(), new.find(BACK).unwrap(), new.find(3).unwrap());
    new.nodes[alpha].selected = false;
    new.nodes[beta].selected = true;
    new.nodes[bar].children.retain(|&c| c != gone);
    let mut out = Vec::new();
    changes(WIN, Some(&old), &new, &mut out);
    let seen: Vec<(Ref, &str, &str, i32)> = out.iter().map(|e| (e.at.clone(), e.member, e.detail, e.d1)).collect();
    let node = |id| Ref::Node(WIN, id);
    assert!(seen.contains(&(node(3), "ChildrenChanged", "remove", 0)));
    assert!(seen.contains(&(node(BETA), "PropertyChange", "accessible-name", 0)));
    assert!(seen.contains(&(node(ALPHA), "StateChanged", "selected", 0)));
    assert!(seen.contains(&(node(BETA), "StateChanged", "selected", 1)));
    assert!(seen.contains(&(node(7), "SelectionChanged", "", 0)));
    assert!(seen.contains(&(node(7), "StateChanged", "focused", 0)));
    assert!(seen.contains(&(node(BETA), "StateChanged", "focused", 1)));
    assert_eq!(out.last().map(|e| (e.iface, e.member)), Some((FOCUS_EVENTS, "Focus")));
    let mut quiet = Vec::new();
    changes(WIN, Some(&new), &new, &mut quiet);
    assert!(quiet.is_empty());
}

#[test]
fn nodes_answer_accessible_and_table_calls() {
    let mut grid = Tree::new("w", Rect::new(0.0, 0.0, 400.0, 300.0));
    let g = grid.add(0, 7, Role::DataGrid, "Files", Rect::new(0.0, 0.0, 400.0, 300.0));
    let h = grid.add(g, 8, Role::Header, "Columns", Rect::new(0.0, 0.0, 400.0, 20.0));
    grid.add(h, 0x80, Role::HeaderItem, "Name", Rect::new(0.0, 0.0, 200.0, 20.0));
    grid.add(h, 0x81, Role::HeaderItem, "Size", Rect::new(200.0, 0.0, 200.0, 20.0));
    let r = grid.add(g, 0x90, Role::DataItem, "a.txt", Rect::new(0.0, 20.0, 400.0, 20.0));
    grid.nodes[r].selected = true;
    grid.add(r, 0x91, Role::Text, "3 KB", Rect::new(200.0, 20.0, 200.0, 20.0));
    let hub = Hub { windows: vec![Win { id: WIN, origin: (100, 50), tree: grid }], events: Vec::new() };
    let fired = std::cell::RefCell::new(Vec::new());
    let act = |w: WindowId, e: Event| fired.borrow_mut().push((w, e));
    let mut id = 0;
    let mut ask = |path: &str, iface: &str, member: &str, args: &[Arg]| Answer { hub: &hub, parent: &Ref::Null, app_id: &mut id, act: &act }.call(path, iface, member, args).unwrap();
    let grid_path = Ref::Node(WIN, 7).path();
    assert_eq!(ask(ROOT_PATH, ACCESSIBLE, "GetChildren", &[]), vec![V::Refs(vec![Ref::Node(WIN, ROOT_ID)])]);
    assert_eq!(ask(&grid_path, ACCESSIBLE, "GetRole", &[]), vec![V::U(55)]);
    assert_eq!(ask(&grid_path, TABLE, "GetAccessibleAt", &[Arg::I(0), Arg::I(1)]), vec![V::Ref(Ref::Node(WIN, 0x91))]);
    assert_eq!(ask(&grid_path, TABLE, "GetColumnHeader", &[Arg::I(1)]), vec![V::Ref(Ref::Node(WIN, 0x81))]);
    assert_eq!(ask(&grid_path, PROPERTIES, "Get", &[Arg::S(TABLE.into()), Arg::S("NColumns".into())]), vec![V::Var(Box::new(V::I(2)))]);
    assert_eq!(ask(&grid_path, TABLE, "GetSelectedRows", &[]), vec![V::Ints(vec![0])]);
    assert_eq!(ask(&Ref::Node(WIN, 0x91).path(), ACCESSIBLE, "GetRoleName", &[]), vec![V::S("table cell".into())]);
    assert_eq!(ask(&Ref::Node(WIN, 0x91).path(), COMPONENT, "GetExtents", &[Arg::I(0)]), vec![V::Tuple(vec![V::I(300), V::I(70), V::I(200), V::I(20)])]);
    assert_eq!(ask(&grid_path, COMPONENT, "GetAccessibleAtPoint", &[Arg::I(310), Arg::I(75), Arg::I(0)]), vec![V::Ref(Ref::Node(WIN, 0x91))]);
    assert_eq!(ask(&Ref::Node(WIN, 0x90).path(), ACTION, "DoAction", &[Arg::I(1)]), vec![V::B(true)]);
    assert_eq!(ask(&grid_path, SELECTION, "SelectChild", &[Arg::I(1)]), vec![V::B(true)]);
    assert_eq!(fired.borrow().as_slice(), [(WIN, Event::A11ySelect(0x90)), (WIN, Event::A11ySelect(0x90))]);
}

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const CLIENT: &str = r#"
import sys, gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi, GLib

def say(*parts):
    print(*parts, flush=True)

desk = Atspi.get_desktop(0)
app = None
for _ in range(50):
    apps = [desk.get_child_at_index(i) for i in range(desk.get_child_count())]
    app = next((a for a in apps if a is not None and a.get_name() == 'Flit'), None)
    if app:
        break
    GLib.usleep(100000)
if app is None:
    say('no app')
    sys.exit(1)
say('app', app.get_role_name(), app.get_child_count())

def walk(node, depth):
    say('node', depth, node.get_role_name(), node.get_name())
    for i in range(node.get_child_count()):
        walk(node.get_child_at_index(i), depth + 1)

frame = app.get_child_at_index(0)
walk(frame, 0)
files = [frame.get_child_at_index(i) for i in range(frame.get_child_count())][-1]
say('list', files.get_role_name(), ','.join(files.get_child_at_index(i).get_name() for i in range(files.get_child_count())))
say('selected', files.get_n_selected_children())
address = frame.get_child_at_index(2)
say('text', address.get_text(0, -1))
back = frame.get_child_at_index(1).get_child_at_index(0)
say('action', Atspi.Action.get_name(back, 0), back.do_action(0))
say('state', frame.get_state_set().contains(Atspi.StateType.ACTIVE))

loop = GLib.MainLoop()
def on_event(e):
    say('event', e.type, e.source.get_name())
    if e.type.startswith('focus'):
        loop.quit()
listener = Atspi.EventListener.new(on_event)
listener.register('object:state-changed:focused')
listener.register('focus:')
GLib.timeout_add(1500, lambda: say('ready') or False)
GLib.timeout_add(15000, lambda: loop.quit() or False)
loop.run()
say('done')
"#;

fn tool(name: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {name}")]).stdout(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn gdbus(address: &str, enabled: bool) {
    let status = Command::new("gdbus")
        .args(["call", "--address", address, "--dest", BUS_NAME, "--object-path", BUS_PATH, "--method", "org.freedesktop.DBus.Properties.Set", "org.a11y.Status", "IsEnabled", &format!("<{enabled}>")])
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

fn wait(what: &str, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_real_atspi_client_walks_invokes_and_hears_focus() {
    if !tool("dbus-daemon") || !tool("gdbus") || !std::path::Path::new("/usr/libexec/at-spi-bus-launcher").exists() {
        eprintln!("skipped: dbus-daemon, gdbus or at-spi2-core is missing");
        return;
    }
    let probe = Command::new("python3").args(["-c", "import gi; gi.require_version('Atspi', '2.0'); from gi.repository import Atspi"]).status();
    if !probe.is_ok_and(|s| s.success()) {
        eprintln!("skipped: python3-gi with Atspi 2.0 is missing");
        return;
    }
    let mut daemon = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("GSETTINGS_BACKEND", "memory")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).unwrap();
    let _daemon = Kill(daemon);
    let address = address.trim().to_owned();
    let (act_tx, act_rx) = crossbeam_channel::unbounded();
    let (refresh_tx, refresh_rx) = crossbeam_channel::unbounded();
    let bus = address.clone();
    std::thread::spawn(move || run(&bus, &move || refresh_tx.send(()).unwrap(), &move |w, e| act_tx.send((w, e)).unwrap()));
    wait("the status probe", || WAKE.load(Ordering::Acquire) >= 0);
    assert!(!listening(), "nothing listens until IsEnabled is set");
    gdbus(&address, true);
    refresh_rx.recv_timeout(Duration::from_secs(10)).expect("the bridge asks for a tree once a client listens");
    assert!(listening());
    publish(WIN, (10, 20), sample(false, "beta"));

    let script = std::env::temp_dir().join(format!("flit-atspi-client-{}.py", std::process::id()));
    std::fs::File::create(&script).unwrap().write_all(CLIENT.as_bytes()).unwrap();
    let mut client = Command::new("python3")
        .arg("-I")
        .arg(&script)
        .env("DBUS_SESSION_BUS_ADDRESS", &address)
        .env("GSETTINGS_BACKEND", "memory")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("AT_SPI_BUS_ADDRESS")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut lines = Vec::new();
    for line in BufReader::new(client.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        if line == "ready" {
            publish(WIN, (10, 20), sample(true, "beta"));
        }
        lines.push(line);
    }
    let _ = client.wait();
    let _ = std::fs::remove_file(&script);
    let text = lines.join("\n");
    let has = |l: &str| lines.iter().any(|x| x == l);
    assert!(has("app application 1"), "{text}");
    assert!(has("node 0 frame Flit test"), "{text}");
    assert!(has("node 1 page tab list Tabs"), "{text}");
    assert!(has("node 2 page tab Home"), "{text}");
    assert!(has("node 2 push button Back"), "{text}");
    assert!(has("node 1 entry Address"), "{text}");
    assert!(has("list list alpha.txt,beta"), "{text}");
    assert!(has("selected 1"), "{text}");
    assert!(has("text /tmp/flit"), "{text}");
    assert!(has("action click True"), "{text}");
    assert!(has("state True"), "{text}");
    assert!(has("event object:state-changed:focused beta"), "{text}");
    assert!(has("event focus: beta"), "{text}");
    assert_eq!(act_rx.recv_timeout(Duration::from_secs(1)).ok(), Some((WIN, Event::Invoke(BACK))), "{text}");

    gdbus(&address, false);
    wait("the bridge to stop listening", || !listening());
}
