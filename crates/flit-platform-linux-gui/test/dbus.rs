use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn methods_turn_uris_into_folders_or_selections() {
    let uris = [b"file:///home/u/a%20b".to_vec(), b"sftp://host/x".to_vec()];
    let folders = request("ShowFolders", &uris).unwrap();
    assert_eq!((folders.paths, folders.select.len(), folders.target), (vec![b"/home/u/a b".to_vec()], 0, OpenTarget::NewTab));
    let items = request("ShowItems", &[b"file:///etc/hosts".to_vec()]).unwrap();
    assert_eq!((items.paths.len(), items.select), (0, vec![b"/etc/hosts".to_vec()]));
    assert!(request("ShowItemProperties", &[b"file:///etc".to_vec()]).is_some());
    assert!(request("Other", &[b"file:///etc".to_vec()]).is_none());
    assert!(request("ShowFolders", &[b"sftp://h/".to_vec()]).is_none());
}

#[test]
fn a_private_session_bus_delivers_show_items_to_the_open_sink() {
    let Ok(mut daemon) = Command::new("dbus-daemon").args(["--session", "--nofork", "--print-address=1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else { return };
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).unwrap();
    let address = address.trim().to_owned();
    let (tx, rx) = crossbeam_channel::unbounded();
    let sink: OpenSink = Arc::new(Mutex::new(Some(tx)));
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    let bus = address.clone();
    std::thread::spawn(move || serve(&bus, sink, Arc::new(|| {}), move |ok| ready_tx.send(ok).unwrap()));
    let ok = ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    if !ok {
        let _ = daemon.kill();
    }
    assert!(ok, "the name was not acquired on the private bus");
    let out = Command::new("dbus-send")
        .args([&format!("--bus={address}"), "--print-reply", "--dest=org.freedesktop.FileManager1", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems", "array:string:file:///tmp/x%20y.txt", "string:"])
        .output()
        .unwrap();
    let open = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let _ = daemon.kill();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(open.select, [b"/tmp/x y.txt".to_vec()]);
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
