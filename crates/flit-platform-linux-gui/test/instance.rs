use super::*;
use crate::temp::Temp;
use crossbeam_channel::unbounded;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

fn child(test: &str) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", test, "--include-ignored", "--nocapture", "--test-threads", "1"]);
    cmd
}

fn args(list: &[&[u8]]) -> Vec<Vec<u8>> {
    list.iter().map(|a| a.to_vec()).collect()
}

#[test]
fn linux_arguments_take_uris_select_and_targets() {
    let open = parse_args(&args(&[b"file:///home/ann/My%20Files", b"--select", b"/home/ann/a,b.txt", b"--new-window", b"relative/dir", b"--select=file://localhost/tmp/x%FF", b"--", b"--odd-name", b"/n"]), Some(b"/home/ann".to_vec()));
    assert_eq!(open.paths, args(&[b"/home/ann/My Files", b"relative/dir", b"--odd-name", b"/n"]));
    assert_eq!(open.select, args(&[b"/home/ann/a,b.txt", b"/tmp/x\xFF"]));
    assert_eq!(open.target, OpenTarget::NewWindow);
    assert_eq!(open.cwd.as_deref(), Some(&b"/home/ann"[..]));
    let places = parse_args(&args(&[b"trash:///", b"computer:///", b"file://elsewhere/etc"]), None);
    assert_eq!(places.paths, args(&[RECYCLE_BIN_SHELL.as_bytes(), b"shell:MyComputerFolder", b"file://elsewhere/etc"]));
    let tag = parse_args(&args(&[b"--tag", b"Red", b"file:///tmp/a%20b"]), None);
    assert_eq!(tag.tag.as_deref(), Some("Red"));
    assert_eq!(tag.paths, args(&[b"/tmp/a b"]));
}

#[test]
#[ignore]
fn forward_child() {
    let Some(dir) = std::env::var_os("FLIT_TEST_RUNTIME") else { return };
    let open = parse_args(&args(&[b"file:///tmp/flit-forwarded%20here", b"--select", b"/tmp/flit-picked"]), None);
    let started = Instant::now();
    match SocketInstance::acquire_in(Path::new(&dir), &open) {
        Instance::Forwarded => println!("forwarded in {:?}", started.elapsed()),
        Instance::Primary(_) => panic!("the child became the primary instance"),
        Instance::Standalone(e) => panic!("standalone: {e}"),
    }
}

#[test]
fn second_process_forwards_its_open_to_the_first() {
    let t = Temp::new("instance");
    let Instance::Primary(primary) = SocketInstance::acquire_in(t.path(), &Open::default()) else { panic!("the first process must be primary") };
    let (lock, socket) = endpoints(t.path());
    assert!(lock.exists());
    assert_eq!(std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777, 0o600);
    let (tx, rx) = unbounded();
    let sink: OpenSink = Arc::new(Mutex::new(Some(tx)));
    let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = woken.clone();
    primary.listen(sink, Arc::new(move || {
        w.fetch_add(1, Ordering::Relaxed);
    }));
    let out = child("instance::tests::forward_child").env("FLIT_TEST_RUNTIME", t.path()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let open = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(open.paths, args(&[b"/tmp/flit-forwarded here"]));
    assert_eq!(open.select, args(&[b"/tmp/flit-picked"]));
    assert_eq!(woken.load(Ordering::Relaxed), 1);
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(line) = text.lines().find_map(|l| l.find("forwarded in").map(|k| &l[k..])) {
        eprintln!("measure: {line}");
    }
}

#[test]
fn a_dead_primary_leaves_no_lock_behind() {
    let t = Temp::new("instance-stale");
    let (_, socket) = endpoints(t.path());
    std::fs::write(&socket, b"stale").unwrap();
    let first = SocketInstance::acquire_in(t.path(), &Open::default());
    assert!(matches!(first, Instance::Primary(_)));
    drop(first);
    assert!(matches!(SocketInstance::acquire_in(t.path(), &Open::default()), Instance::Primary(_)));
}

#[test]
fn foreign_bytes_are_rejected_without_an_open() {
    let t = Temp::new("instance-junk");
    let Instance::Primary(primary) = SocketInstance::acquire_in(t.path(), &Open::default()) else { panic!() };
    let (tx, rx) = unbounded();
    primary.listen(Arc::new(Mutex::new(Some(tx))), Arc::new(|| {}));
    let mut s = UnixStream::connect(endpoints(t.path()).1).unwrap();
    assert_eq!(peer_uid(&s).unwrap(), unsafe { libc::getuid() });
    s.write_all(&[8, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
    let mut reply = Vec::new();
    let _ = s.read_to_end(&mut reply);
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
#[ignore]
fn signal_child() {
    let Some(save) = std::env::var_os("FLIT_TEST_SAVE") else { return };
    on_shutdown_signal(move |sig| {
        std::fs::write(&save, sig.to_string()).unwrap();
        std::process::exit(0);
    })
    .unwrap();
    println!("ready");
    std::thread::sleep(Duration::from_secs(10));
    std::process::exit(3);
}

fn signal_run(sig: libc::c_int) {
    let t = Temp::new("signal");
    let save = t.path().join("session");
    let mut c = child("instance::tests::signal_child").env("FLIT_TEST_SAVE", &save).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(c.stdout.take().unwrap()).lines();
    assert!(lines.any(|l| l.is_ok_and(|l| l.ends_with("ready"))));
    unsafe { libc::kill(c.id() as libc::pid_t, sig) };
    let status = c.wait().unwrap();
    assert!(status.success(), "{status:?}");
    assert_eq!(std::fs::read_to_string(&save).unwrap(), sig.to_string());
}

#[test]
fn shutdown_signals_trigger_the_save_handler() {
    for sig in SHUTDOWN_SIGNALS {
        signal_run(sig);
    }
}
