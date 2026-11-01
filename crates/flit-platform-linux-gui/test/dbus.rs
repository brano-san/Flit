use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static STARTUP_TOKEN_USERS: Mutex<()> = Mutex::new(());

#[test]
fn methods_turn_uris_into_folders_or_selections() {
    let uris = [b"file:///home/u/a%20b".to_vec(), b"sftp://host/x".to_vec()];
    let folders = request("ShowFolders", &uris).unwrap();
    assert_eq!((folders.paths, folders.select.len(), folders.target, folders.properties), (vec![b"/home/u/a b".to_vec()], 0, OpenTarget::NewTab, false));
    let items = request("ShowItems", &[b"file:///etc/hosts".to_vec()]).unwrap();
    assert_eq!((items.paths.len(), items.select, items.properties), (0, vec![b"/etc/hosts".to_vec()], false));
    let props = request("ShowItemProperties", &[b"file:///etc".to_vec()]).unwrap();
    assert_eq!((props.paths.len(), props.select, props.properties), (0, vec![b"/etc".to_vec()], true));
    assert!(request("Other", &[b"file:///etc".to_vec()]).is_none());
    assert!(request("ShowFolders", &[b"sftp://h/".to_vec()]).is_none());
}

fn private_bus() -> Option<(Child, String)> {
    let mut daemon = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).unwrap();
    Some((daemon, address.trim().to_owned()))
}

fn serving(address: &str) -> (crossbeam_channel::Receiver<Open>, bool) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let sink: OpenSink = Arc::new(Mutex::new(Some(tx)));
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    let bus = address.to_owned();
    std::thread::spawn(move || serve(&bus, sink, Arc::new(|| {}), move |ok| ready_tx.send(ok).unwrap()));
    (rx, ready_rx.recv_timeout(Duration::from_secs(5)).unwrap())
}

fn call(address: &str, method: &str, uri: &str, startup_id: &str) -> std::process::Output {
    Command::new("dbus-send")
        .args([&format!("--bus={address}"), "--print-reply", "--dest=org.freedesktop.FileManager1", "/org/freedesktop/FileManager1", &format!("org.freedesktop.FileManager1.{method}"), &format!("array:string:{uri}"), &format!("string:{startup_id}")])
        .output()
        .unwrap()
}

#[test]
fn a_private_session_bus_delivers_show_items_to_the_open_sink() {
    let Some((mut daemon, address)) = private_bus() else { return };
    let _serial = STARTUP_TOKEN_USERS.lock().unwrap_or_else(|e| e.into_inner());
    let (rx, ok) = serving(&address);
    if !ok {
        let _ = daemon.kill();
    }
    assert!(ok, "the name was not acquired on the private bus");
    let out = call(&address, "ShowItems", "file:///tmp/x%20y.txt", "flit-test_TIME42");
    let open = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let token = crate::window::take_startup_token();
    let props = call(&address, "ShowItemProperties", "file:///tmp/z", "");
    let shown = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let no_token = crate::window::take_startup_token();
    let _ = daemon.kill();
    assert!(out.status.success() && props.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(open.select, [b"/tmp/x y.txt".to_vec()]);
    assert_eq!(token.as_deref(), Some("flit-test_TIME42"));
    assert_eq!((shown.select, shown.properties, no_token), (vec![b"/tmp/z".to_vec()], true, None));
}

fn names_owner(address: &str) -> String {
    let out = Command::new("dbus-send")
        .args([&format!("--bus={address}"), "--print-reply=literal", "--dest=org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.GetNameOwner", "string:org.freedesktop.FileManager1"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn a_name_held_by_another_file_manager_is_left_alone() {
    let Some((mut daemon, address)) = private_bus() else { return };
    let Some(api) = load() else {
        let _ = daemon.kill();
        return;
    };
    let addr = CString::new(address.clone()).unwrap();
    let other = unsafe {
        let mut err = Error::new();
        (api.error_init)(&mut err);
        let conn = (api.open_private)(addr.as_ptr(), &mut err);
        assert!(!conn.is_null() && (api.register)(conn, &mut err) != 0);
        const ALLOW_REPLACEMENT: u32 = 1;
        assert_eq!((api.request_name)(conn, NAME.as_ptr(), ALLOW_REPLACEMENT, &mut err), PRIMARY_OWNER);
        conn
    };
    let owner = names_owner(&address);
    let (_rx, ok) = serving(&address);
    let still = names_owner(&address);
    let queued = Command::new("dbus-send")
        .args([&format!("--bus={address}"), "--print-reply=literal", "--dest=org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.ListQueuedOwners", "string:org.freedesktop.FileManager1"])
        .output()
        .unwrap();
    unsafe {
        (api.close)(other);
        (api.release)(other);
    }
    let _ = daemon.kill();
    assert!(!ok, "Flit took a name another file manager holds");
    assert!(!owner.is_empty() && owner == still, "{owner} -> {still}");
    let queue = String::from_utf8_lossy(&queued.stdout);
    assert_eq!(queue.split_whitespace().filter(|w| w.starts_with(':')).count(), 1, "{queue}");
}

#[test]
fn a_call_queued_by_bus_activation_is_answered() {
    let dir = crate::temp::Temp::new("dbus-activation");
    let services = dir.path().join("services");
    std::fs::create_dir_all(&services).unwrap();
    std::fs::write(services.join("org.freedesktop.FileManager1.service"), "[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec=/bin/sleep 30\n").unwrap();
    let config = dir.path().join("bus.conf");
    std::fs::write(
        &config,
        format!(
            "<busconfig><include>/usr/share/dbus-1/session.conf</include><servicedir>{}</servicedir></busconfig>",
            services.display()
        ),
    )
    .unwrap();
    let Ok(mut daemon) = Command::new("dbus-daemon").arg(format!("--config-file={}", config.display())).args(["--nofork", "--print-address=1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else { return };
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).unwrap();
    let address = address.trim().to_owned();
    let _serial = STARTUP_TOKEN_USERS.lock().unwrap_or_else(|e| e.into_inner());
    let send = Command::new("dbus-send")
        .args([&format!("--bus={address}"), "--print-reply", "--reply-timeout=5000", "--dest=org.freedesktop.FileManager1", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowFolders", "array:string:file:///srv", "string:"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let (tx, rx) = crossbeam_channel::unbounded();
    let sink: OpenSink = Arc::new(Mutex::new(Some(tx)));
    let bus = address.clone();
    std::thread::spawn(move || serve(&bus, sink, Arc::new(|| {}), |_| {}));
    let open = rx.recv_timeout(Duration::from_secs(5));
    let out = send.wait_with_output().unwrap();
    let _ = daemon.kill();
    assert_eq!(open.unwrap().paths, [b"/srv".to_vec()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}
