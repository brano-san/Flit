use super::*;
use flit_core::render::Rect;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const WIN: WindowId = 7;
const BACK: u64 = 0x10;
const ALPHA: u64 = 0x21;
const BETA: u64 = 0x22;

#[test]
fn button_layout_signals_preserve_empty_strings_and_ignore_other_settings() {
    let s = |v: &str| Arg::S(v.into());
    assert_eq!(look_change(&[s(GNOME_WM), s("button-layout"), Arg::L(vec![s("")])]), Some(LookChange::WindowButtons(String::new())));
    assert_eq!(look_change(&[s(GNOME_WM), s("button-layout"), Arg::L(vec![Arg::L(vec![s("close:")])])]), Some(LookChange::WindowButtons("close:".into())));
    assert_eq!(look_change(&[s(GNOME_WM), s("theme"), s("close:")]), None);
    assert_eq!(look_change(&[s(APPEARANCE), s("button-layout"), s("close:")]), None);
    assert_eq!(look_change(&[s(GNOME_WM), s("button-layout"), Arg::I(1)]), None);
}

#[test]
fn button_layout_changes_arrive_from_a_private_portal_bus_including_no_buttons() {
    if !tool("dbus-daemon") || !tool("dbus-send") { return; }
    let mut daemon = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).unwrap();
    let daemon = Kill(daemon);
    let address = address.trim().to_owned();
    let (tx, rx) = crossbeam_channel::unbounded();
    let bus = address.clone();
    let watcher = std::thread::spawn(move || watch_settings(&bus, &|_| {}, &|_| {}, &move |change| { let _ = tx.send(change); }));
    let emit = |value: &str| {
        assert!(Command::new("dbus-send").args([&format!("--bus={address}"), "--type=signal", "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Settings.SettingChanged", &format!("string:{GNOME_WM}"), "string:button-layout", &format!("variant:string:{value}")]).status().unwrap().success());
    };
    let until = Instant::now() + Duration::from_secs(10);
    let mut heard = None;
    while heard.is_none() && Instant::now() < until {
        emit("close:");
        heard = rx.recv_timeout(Duration::from_millis(100)).ok();
    }
    assert_eq!(heard, Some(LookChange::WindowButtons("close:".into())));
    emit("");
    let empty = (0..50).find_map(|_| rx.recv_timeout(Duration::from_millis(100)).ok().filter(|v| *v == LookChange::WindowButtons(String::new())));
    assert_eq!(empty, Some(LookChange::WindowButtons(String::new())));
    drop(daemon);
    watcher.join().unwrap();
}

#[test]
fn the_private_settings_read_reply_unwraps_both_variants_and_preserves_empty() {
    if !tool("dbus-daemon") { return; }
    let mut daemon = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).unwrap();
    let daemon = Kill(daemon);
    let address = address.trim().to_owned();
    let bus = address.clone();
    let (tx, rx) = crossbeam_channel::bounded(1);
    let server = std::thread::spawn(move || {
        let api = dbus::load().unwrap();
        let conn = Conn::open(&api, &bus).unwrap();
        let owned = conn.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName", &[V::S(PORTAL.to_owned()), V::U(0)]).unwrap();
        assert_eq!(owned, vec![Arg::I(1)]);
        tx.send(()).unwrap();
        let mut answered = 0;
        let until = Instant::now() + Duration::from_secs(10);
        while answered < 2 && Instant::now() < until {
            conn.drain(|message| unsafe {
                if (api.msg_type)(message) == TYPE_METHOD_CALL && text((api.member)(message)) == "Read" {
                    assert_eq!(args(&api, message), vec![Arg::S(GNOME_WM.to_owned()), Arg::S("button-layout".to_owned())]);
                    let reply = (api.new_return)(message);
                    let value = if answered == 0 { "close:" } else { "" };
                    conn.append(reply, &[V::Var(Box::new(V::Var(Box::new(V::S(value.to_owned())))))]);
                    conn.send(reply);
                    conn.flush();
                    answered += 1;
                }
            });
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(answered, 2);
    });
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(read_button_layout_at(&address), Some("close:".into()));
    assert_eq!(read_button_layout_at(&address), Some(String::new()));
    server.join().unwrap();
    drop(daemon);
}

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
say('action', back.get_action_name(0), back.do_action(0))
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

#[test]
fn only_the_appearance_color_scheme_setting_counts() {
    let s = |v: &str| Arg::S(v.into());
    assert_eq!(color_scheme_change(&[s(APPEARANCE), s("color-scheme"), Arg::L(vec![Arg::I(1)])]), Some(1));
    assert_eq!(color_scheme_change(&[s(APPEARANCE), s("accent-color"), Arg::L(vec![Arg::I(1)])]), None);
    assert_eq!(color_scheme_change(&[s("org.gnome.desktop.interface"), s("color-scheme"), Arg::L(vec![Arg::I(1)])]), None);
}

#[test]
fn the_portal_setting_changed_signal_switches_the_theme_live() {
    if !tool("dbus-daemon") || !tool("dbus-send") {
        eprintln!("skipped: dbus-daemon or dbus-send is missing");
        return;
    }
    let mut daemon = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).unwrap();
    let daemon = Kill(daemon);
    let address = address.trim().to_owned();
    let (tx, rx) = crossbeam_channel::unbounded();
    let bus = address.clone();
    let watcher = std::thread::spawn(move || watch_settings(&bus, &move |scheme| tx.send(scheme).unwrap(), &|_| {}, &|_| {}));
    let signal = |scheme: u32| {
        Command::new("dbus-send")
            .args([&format!("--bus={address}"), "--type=signal", "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Settings.SettingChanged", "string:org.freedesktop.appearance", "string:color-scheme", &format!("variant:uint32:{scheme}")])
            .status()
            .unwrap()
    };
    let mut heard = None;
    let until = Instant::now() + Duration::from_secs(10);
    while heard.is_none() && Instant::now() < until {
        assert!(signal(1).success());
        heard = rx.recv_timeout(Duration::from_millis(100)).ok();
    }
    assert_eq!(heard, Some(1));
    assert!(signal(2).success());
    let next = (0..50).find_map(|_| rx.recv_timeout(Duration::from_millis(200)).ok().filter(|&s| s != 1));
    assert_eq!(next, Some(2));
    drop(daemon);
    watcher.join().unwrap();
}

#[test]
fn only_the_gnome_mouse_double_click_setting_counts() {
    let s = |v: &str| Arg::S(v.into());
    assert_eq!(double_click_change(&[s(MOUSE), s("double-click"), Arg::L(vec![Arg::I(550)])]), Some(550));
    assert_eq!(double_click_change(&[s(MOUSE), s("speed"), Arg::L(vec![Arg::I(1)])]), None);
    assert_eq!(double_click_change(&[s(APPEARANCE), s("double-click"), Arg::L(vec![Arg::I(550)])]), None);
    assert_eq!(color_scheme_change(&[s(MOUSE), s("double-click"), Arg::L(vec![Arg::I(550)])]), None);
}

#[test]
fn a_changed_gnome_double_click_time_is_heard_live_and_other_settings_are_not() {
    if !tool("dbus-daemon") || !tool("dbus-send") {
        eprintln!("skipped: dbus-daemon or dbus-send is missing");
        return;
    }
    let mut daemon = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).unwrap();
    let daemon = Kill(daemon);
    let address = address.trim().to_owned();
    let (tx, rx) = crossbeam_channel::unbounded();
    let bus = address.clone();
    let watcher = std::thread::spawn(move || watch_settings(&bus, &|_| {}, &move |ms| tx.send(ms).unwrap(), &|_| {}));
    let signal = |ns: &str, key: &str, value: &str| {
        Command::new("dbus-send")
            .args([&format!("--bus={address}"), "--type=signal", "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Settings.SettingChanged", &format!("string:{ns}"), &format!("string:{key}"), value])
            .status()
            .unwrap()
    };
    let mut heard = None;
    let until = Instant::now() + Duration::from_secs(10);
    while heard.is_none() && Instant::now() < until {
        assert!(signal(MOUSE, "double-click", "variant:int32:550").success());
        heard = rx.recv_timeout(Duration::from_millis(100)).ok();
    }
    assert_eq!(heard, Some(550));
    while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
    assert!(signal(APPEARANCE, "color-scheme", "variant:uint32:1").success());
    assert!(signal(MOUSE, "speed", "variant:double:0.5").success());
    assert!(signal(MOUSE, "double-click", "variant:int32:300").success());
    let next = (0..50).find_map(|_| rx.recv_timeout(Duration::from_millis(200)).ok());
    assert_eq!(next, Some(300), "the color scheme and the pointer speed are not double-click times");
    drop(daemon);
    watcher.join().unwrap();
}

#[test]
fn accent_and_text_size_changes_are_heard_and_other_settings_are_not() {
    let s = |v: &str| Arg::S(v.into());
    let rgb = Arg::L(vec![Arg::L(vec![Arg::D(1.0), Arg::D(0.5), Arg::D(0.0)])]);
    let orange = flit_core::theme::Accent::single(flit_core::render::Color::rgb(0xFF8000));
    assert_eq!(look_change(&[s(APPEARANCE), s("accent-color"), rgb]), Some(LookChange::Accent(Some(orange))));
    assert_eq!(look_change(&[s(APPEARANCE), s("accent-color"), Arg::L(vec![Arg::D(-1.0), Arg::D(-1.0), Arg::D(-1.0)])]), Some(LookChange::Accent(None)), "an unset accent brings Flit's own back");
    assert_eq!(look_change(&[s(GNOME_INTERFACE), s("text-scaling-factor"), Arg::L(vec![Arg::D(1.25)])]), Some(LookChange::TextScale(1.25)));
    assert_eq!(look_change(&[s(APPEARANCE), s("color-scheme"), Arg::L(vec![Arg::I(1)])]), None);
    assert_eq!(look_change(&[s(GNOME_INTERFACE), s("font-name"), Arg::L(vec![s("Cantarell 11")])]), None);
}

#[test]
fn the_file_chooser_portal_is_asked_for_images_or_a_folder_and_its_answer_is_read() {
    assert_eq!(request_path(":1.42", "flit7_0"), "/org/freedesktop/portal/desktop/request/1_42/flit7_0");
    let options = chooser_options("flit7_0", &["jpg", "png"], false, Some(b"/home/u/Pictures"), "*.jpg *.png");
    let keys: Vec<&str> = options.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["handle_token", "modal", "directory", "filters", "current_folder"]);
    assert_eq!(options[3].1.sig(), "a(sa(us))");
    assert!(matches!(&options[4].1, V::Bytes(b) if b.as_slice() == b"/home/u/Pictures\0"), "the start folder is a NUL-terminated byte string");
    let s = |v: &str| Arg::S(v.into());
    let answer = [Arg::I(0), Arg::L(vec![Arg::L(vec![s("uris"), Arg::L(vec![Arg::L(vec![s("file:///home/u/Pictures/sea%20side.jpg")])])]), Arg::L(vec![s("choices"), Arg::L(vec![])])])];
    assert_eq!(chosen_uri(&answer).as_deref(), Some("file:///home/u/Pictures/sea%20side.jpg"));
}

#[test]
fn a_cancelled_chooser_or_a_folder_pick_carries_no_file_filter() {
    let s = |v: &str| Arg::S(v.into());
    assert_eq!(chosen_uri(&[Arg::I(1), Arg::L(vec![])]), None, "Cancel picks nothing");
    assert_eq!(chosen_uri(&[Arg::I(0), Arg::L(vec![Arg::L(vec![s("uris"), Arg::L(vec![])])])]), None);
    let folder = chooser_options("t", &[], true, None, "");
    assert_eq!(folder.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["handle_token", "modal", "directory"]);
    assert!(matches!(folder[2].1, V::B(true)));
}

#[test]
fn an_icon_theme_change_is_heard_with_its_name() {
    let s = |v: &str| Arg::S(v.into());
    assert_eq!(look_change(&[s(GNOME_INTERFACE), s("icon-theme"), Arg::L(vec![s("Papirus")])]), Some(LookChange::IconTheme("Papirus".into())));
    assert_eq!(look_change(&[s(GNOME_INTERFACE), s("icon-theme"), Arg::L(vec![s("")])]), None);
}

#[test]
fn font_rendering_changes_of_gnome_are_heard() {
    let s = |v: &str| Arg::S(v.into());
    assert_eq!(look_change(&[s(GNOME_INTERFACE), s("font-antialiasing"), Arg::L(vec![s("rgba")])]), Some(LookChange::FontRendering("font-antialiasing".into(), "rgba".into())));
    assert_eq!(look_change(&[s(GNOME_INTERFACE), s("font-rgba-order"), Arg::L(vec![s("bgr")])]), Some(LookChange::FontRendering("font-rgba-order".into(), "bgr".into())));
    assert_eq!(look_change(&[s(GNOME_INTERFACE), s("font-hinting"), Arg::L(vec![s("slight")])]), Some(LookChange::FontRendering("font-hinting".into(), "slight".into())));
    assert_eq!(look_change(&[s(GNOME_WM), s("font-hinting"), Arg::L(vec![s("slight")])]), None, "only the interface namespace holds font settings");
}
