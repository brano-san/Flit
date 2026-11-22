use super::*;
use crate::dbus::Api;
use flit_core::provider::{wipe, Password, ProviderEvent, ASK_PASSWORD, ASK_SAVING};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

fn spec_of(uri: &str) -> (String, Vec<(&'static str, String)>) {
    let s = spec(uri).unwrap_or_else(|| panic!("no spec for {uri}"));
    (String::from_utf8(s.prefix).unwrap(), s.items.into_iter().map(|(k, v)| (k, String::from_utf8(v).unwrap())).collect())
}

fn items(pairs: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    pairs.iter().map(|(k, v)| (*k, (*v).to_owned())).collect()
}

#[test]
fn smb_addresses_become_share_and_server_specs_with_the_folder_as_prefix() {
    assert_eq!(spec_of("smb://NAS/Share"), ("/".into(), items(&[("server", "nas"), ("share", "share"), ("type", "smb-share")])));
    assert_eq!(spec_of("smb://nas/share/Docs/2024/"), ("/Docs/2024".into(), items(&[("server", "nas"), ("share", "share"), ("type", "smb-share")])));
    assert_eq!(spec_of("smb://nas"), ("/".into(), items(&[("server", "nas"), ("type", "smb-server")])));
    assert_eq!(spec_of("smb://WORK;ann@nas:4455/share"), ("/".into(), items(&[("domain", "WORK"), ("port", "4455"), ("server", "nas"), ("share", "share"), ("type", "smb-share"), ("user", "ann")])));
    assert_eq!(spec_of("smb://ann@nas:445/a%20b/c%20d"), ("/c d".into(), items(&[("server", "nas"), ("share", "a b"), ("type", "smb-share"), ("user", "ann")])), "the default port is left out and names are decoded");
}

#[test]
fn sftp_addresses_carry_user_host_port_and_path() {
    assert_eq!(spec_of("sftp://user@host:2222/path"), ("/path".into(), items(&[("host", "host"), ("port", "2222"), ("type", "sftp"), ("user", "user")])));
    assert_eq!(spec_of("ssh://Host.Example:22/"), ("/".into(), items(&[("host", "host.example"), ("type", "sftp")])), "ssh is the sftp alias and port 22 is the default");
    assert_eq!(spec_of("sftp://[FE80::1]:2222/srv/./x/../y"), ("/srv/y".into(), items(&[("host", "fe80::1"), ("port", "2222"), ("type", "sftp")])));
    assert_eq!(spec_of("sftp://me%40work@host"), ("/".into(), items(&[("host", "host"), ("type", "sftp"), ("user", "me@work")])));
}

#[test]
fn webdav_addresses_keep_ssl_and_only_a_custom_port() {
    assert_eq!(spec_of("dav://host/remote.php/webdav"), ("/remote.php/webdav".into(), items(&[("host", "host"), ("ssl", "false"), ("type", "dav")])));
    assert_eq!(spec_of("davs://ann@host:443/"), ("/".into(), items(&[("host", "host"), ("ssl", "true"), ("type", "dav"), ("user", "ann")])));
    assert_eq!(spec_of("davs://host:8443/x"), ("/x".into(), items(&[("host", "host"), ("port", "8443"), ("ssl", "true"), ("type", "dav")])));
    assert_eq!(spec_of("dav://host:443/"), ("/".into(), items(&[("host", "host"), ("port", "443"), ("ssl", "false"), ("type", "dav")])));
}

#[test]
fn discovered_webdav_ftp_and_nfs_addresses_have_native_mount_specs() {
    for (uri, prefix, fields) in [
        ("dav+sd://Team%20DAV._webdav._tcp.local", "/", vec![("host", "Team DAV._webdav._tcp.local"), ("type", "dav+sd")]),
        ("davs+sd://Work%252eDAV._webdavs._tcp.local/docs", "/docs", vec![("host", "Work%2eDAV._webdavs._tcp.local"), ("type", "davs+sd")]),
        ("ftp://user@BOX:21/public", "/public", vec![("host", "box"), ("type", "ftp"), ("user", "user")]),
        ("ftp://[fd00::2]:2121/", "/", vec![("host", "fd00::2"), ("port", "2121"), ("type", "ftp")]),
        ("nfs://files:2049/srv/export/folder", "/srv/export/folder", vec![("host", "files"), ("port", "2049"), ("type", "nfs")]),
    ] {
        let parsed = spec(uri).expect(uri);
        assert_eq!(parsed.prefix, prefix.as_bytes(), "{uri}");
        assert_eq!(parsed.items, fields.into_iter().map(|(k, v)| (k, v.as_bytes().to_vec())).collect::<Vec<_>>(), "{uri}");
    }
    for uri in ["ftp:///no-host", "nfs://", "afp://host/x", "smb://", "smb://nas/._hidden", "sftp://host:abc/", "sftp://host/?q=1", "sftp://host/a b", "sftp://@/", "http://host/"] {
        assert_eq!(spec(uri), None, "{uri}");
    }
    assert_eq!(server("sftp://ann@files.example:2222/x"), "files.example");
}

#[test]
fn the_budget_stops_while_paused_and_keeps_what_was_left() {
    let t0 = Instant::now();
    let s = Duration::from_secs;
    let mut b = Budget::start(s(45), t0);
    assert_eq!(b.remaining(t0 + s(10)), s(35));
    b.pause(t0 + s(10));
    assert!(!b.expired(t0 + s(600)), "a form that stays open for ten minutes does not use up the wait");
    assert_eq!(b.remaining(t0 + s(600)), s(35));
    b.resume(t0 + s(600));
    assert_eq!(b.remaining(t0 + s(620)), s(15), "after the answer only the rest of the wait is left, not a fresh 45 s");
    assert!(b.expired(t0 + s(635)));
    b.resume(t0 + s(700));
    assert!(b.expired(t0 + s(635)), "resuming twice does not restart the clock");
}

const DO_NOT_QUEUE: u32 = 4;
const PRIMARY_OWNER: c_int = 1;
const FAILED_HANDLED: &str = "org.gtk.GDBus.UnmappedGError.Quark._g_2dio_2derror_2dquark.Code30";
const PERMISSION: &str = "org.gtk.GDBus.UnmappedGError.Quark._g_2dio_2derror_2dquark.Code14";

struct Bus {
    daemon: Child,
    address: String,
    _dir: crate::temp::Temp,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

const BUS_CONFIG: &str = "<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth><policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/><allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>";

fn private_bus() -> Option<Bus> {
    let dir = crate::temp::Temp::new("gvfs-bus");
    let config = dir.path().join("bus.conf");
    std::fs::write(&config, BUS_CONFIG).ok()?;
    let mut daemon = Command::new("dbus-daemon").arg(format!("--config-file={}", config.display())).args(["--nofork", "--print-address=1"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut address = String::new();
    BufReader::new(daemon.stdout.take()?).read_line(&mut address).ok()?;
    Some(Bus { daemon, address: address.trim().to_owned(), _dir: dir })
}

#[derive(Clone, Copy)]
enum Script {
    Password,
    Question,
    Silent,
    AbortWhileAsking,
}

#[derive(Default)]
struct Seen {
    prefix: Vec<u8>,
    spec: Vec<(String, Vec<u8>)>,
    replies: Vec<Vec<Arg>>,
}

fn bytes(a: &Arg) -> Vec<u8> {
    match a {
        Arg::L(v) => v.iter().map(|b| if let Arg::I(n) = b { *n as u8 } else { panic!("not a byte: {b:?}") }).collect(),
        other => panic!("not a byte array: {other:?}"),
    }
}

fn read_spec(a: &Arg, seen: &mut Seen) {
    let Arg::L(parts) = a else { panic!("the spec is not a struct") };
    seen.prefix = bytes(&parts[0]);
    let Arg::L(entries) = &parts[1] else { panic!("the spec has no dictionary") };
    for e in entries {
        let Arg::L(kv) = e else { panic!("not a dictionary entry") };
        let (Arg::S(k), Arg::L(var)) = (&kv[0], &kv[1]) else { panic!("bad entry {kv:?}") };
        seen.spec.push((k.clone(), bytes(&var[0])));
    }
}

fn pump(api: &Api, conn: *mut c_void, waiting: &mut Vec<u32>, held: &mut Vec<*mut c_void>, seen: &Mutex<Seen>, until: Instant) -> Option<*mut c_void> {
    unsafe {
        while Instant::now() < until {
            if (api.read_write)(conn, 20) == 0 {
                return None;
            }
            loop {
                let msg = (api.pop)(conn);
                if msg.is_null() {
                    break;
                }
                let kind = (api.msg_type)(msg);
                if kind == TYPE_METHOD_CALL && dbus::text((api.member)(msg)) == "MountLocation" {
                    return Some(msg);
                }
                if (kind == TYPE_RETURN || kind == TYPE_ERROR) && waiting.contains(&(api.reply_serial)(msg)) {
                    waiting.retain(|s| *s != (api.reply_serial)(msg));
                    seen.lock().unwrap().replies.push(args(api, msg));
                }
                held.push(msg);
            }
            if waiting.is_empty() {
                return None;
            }
        }
        None
    }
}

fn call(api: &Api, conn: *mut c_void, to: &(String, String), member: &str, values: &[V]) -> u32 {
    unsafe {
        let msg = (api.new_method_call)(CString::new(to.0.clone()).unwrap().as_ptr(), CString::new(to.1.clone()).unwrap().as_ptr(), c"org.gtk.vfs.MountOperation".as_ptr(), CString::new(member).unwrap().as_ptr());
        let mut it = Iter::new();
        (api.iter_init_append)(msg, &mut it);
        for v in values {
            put(api, &mut it, "", v);
        }
        let mut serial = 0;
        assert!((api.send)(conn, msg, &mut serial) != 0);
        (api.unref)(msg);
        serial
    }
}

fn finish(api: &Api, conn: *mut c_void, call: *mut c_void, error: Option<(&str, &str)>) {
    unsafe {
        let r = match error {
            Some((name, text)) => (api.new_error)(call, CString::new(name).unwrap().as_ptr(), CString::new(text).unwrap().as_ptr()),
            None => (api.new_return)(call),
        };
        (api.send)(conn, r, std::ptr::null_mut());
        (api.unref)(r);
        (api.flush)(conn);
    }
}

fn fake_gvfs(address: &str, script: Script, password: &'static str) -> (Arc<Mutex<Seen>>, std::thread::JoinHandle<()>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (address, out) = (address.to_owned(), seen.clone());
    let worker = std::thread::spawn(move || {
        let api = dbus::load().expect("libdbus");
        let mut err = dbus::Error::new();
        let addr = CString::new(address).unwrap();
        unsafe {
            (api.error_init)(&mut err);
            let conn = (api.open_private)(addr.as_ptr(), &mut err);
            let owned = !conn.is_null() && (api.register)(conn, &mut err) != 0 && (api.request_name)(conn, c"org.gtk.vfs.Daemon".as_ptr(), DO_NOT_QUEUE, &mut err) == PRIMARY_OWNER;
            (api.error_free)(&mut err);
            ready_tx.send(owned).unwrap();
            if !owned {
                return;
            }
            let (mut waiting, mut held) = (Vec::new(), Vec::new());
            let end = Instant::now() + Duration::from_secs(20);
            let mount = loop {
                if let Some(m) = pump(api, conn, &mut vec![0], &mut held, &out, end) {
                    break m;
                }
                if Instant::now() >= end {
                    return;
                }
            };
            let a = args(api, mount);
            read_spec(&a[0], &mut out.lock().unwrap());
            let Some(Arg::L(source)) = a.get(1) else { panic!("no mount source") };
            let (Arg::S(bus), Arg::S(path)) = (&source[0], &source[1]) else { panic!("bad mount source") };
            let to = (bus.clone(), path.clone());
            let ask_password = || vec![V::S("Password for ann on nas".into()), V::S("ann".into()), V::S(String::new()), V::U(ASK_PASSWORD | ASK_SAVING)];
            match script {
                Script::Silent => {
                    pump(api, conn, &mut vec![0], &mut held, &out, Instant::now() + Duration::from_secs(3));
                }
                Script::Password => {
                    waiting.push(call(api, conn, &to, "AskPassword", &ask_password()));
                    pump(api, conn, &mut waiting, &mut held, &out, end);
                    let reply = out.lock().unwrap().replies.last().cloned().unwrap_or_default();
                    let error = match (reply.get(1), reply.get(2)) {
                        (Some(Arg::I(1)), _) => Some((FAILED_HANDLED, "Password dialog cancelled")),
                        (_, Some(Arg::S(p))) if p == password => None,
                        _ => Some((PERMISSION, "Permission denied")),
                    };
                    finish(api, conn, mount, error);
                }
                Script::Question => {
                    waiting.push(call(api, conn, &to, "AskQuestion", &[V::S("The identity of host cannot be verified".into()), V::Strs(vec!["Log In Anyway".into(), "Cancel Login".into()])]));
                    pump(api, conn, &mut waiting, &mut held, &out, end);
                    let reply = out.lock().unwrap().replies.last().cloned().unwrap_or_default();
                    let picked = matches!((reply.get(1), reply.get(2)), (Some(Arg::I(0)), Some(Arg::I(0))));
                    finish(api, conn, mount, (!picked).then_some((FAILED_HANDLED, "Login dialog cancelled")));
                }
                Script::AbortWhileAsking => {
                    waiting.push(call(api, conn, &to, "AskPassword", &ask_password()));
                    pump(api, conn, &mut vec![0], &mut held, &out, Instant::now() + Duration::from_millis(400));
                    waiting.push(call(api, conn, &to, "Aborted", &[]));
                    pump(api, conn, &mut waiting, &mut held, &out, end);
                    finish(api, conn, mount, Some((FAILED_HANDLED, "Password dialog cancelled")));
                }
            }
            (api.unref)(mount);
            for m in held {
                (api.unref)(m);
            }
            (api.flush)(conn);
            (api.close)(conn);
            (api.release)(conn);
        }
    });
    assert!(ready_rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false), "the fake daemon did not get its name");
    (seen, worker)
}

type Answerer = Box<dyn FnMut(&MountAsk) -> Option<(Duration, MountReply)> + Send>;

#[derive(Default)]
struct Log {
    asks: Vec<(u64, MountAsk)>,
    done: Vec<u64>,
}

fn answering(mut answer: Answerer) -> (MountPrompt, std::thread::JoinHandle<Log>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let prompt = MountPrompt::new(9, tx, Arc::new(|| {}));
    let worker = std::thread::spawn(move || {
        let mut log = Log::default();
        for ev in rx {
            match ev {
                ProviderEvent::MountAsk { tab: 9, ask, answer: a, .. } => {
                    log.asks.push((a.id(), ask.clone()));
                    match answer(&ask) {
                        Some((delay, reply)) => {
                            std::thread::sleep(delay);
                            a.send(reply);
                        }
                        None => drop(a),
                    }
                }
                ProviderEvent::MountAskDone { tab: 9, id } => log.done.push(id),
                other => panic!("unexpected {other:?}"),
            }
        }
        log
    });
    (prompt, worker)
}

fn login(password: &str, remember: bool) -> MountReply {
    MountReply::Login { user: "ann".into(), domain: String::new(), password: Password::new(password.into()), anonymous: false, remember }
}

fn login_as(ask: &MountAsk, password: &str) -> MountReply {
    let MountAsk::Login(l) = ask else { return MountReply::Cancel };
    MountReply::Login { user: l.user.clone(), domain: l.domain.clone(), password: Password::new(password.into()), anonymous: false, remember: false }
}

fn nas() -> Spec {
    spec("smb://ann@nas/share/docs").unwrap()
}

#[test]
fn a_password_typed_in_the_form_reaches_gvfs_and_the_share_is_mounted() {
    let Some(bus) = private_bus() else { return };
    let (seen, daemon) = fake_gvfs(&bus.address, Script::Password, "s3cret");
    let (prompt, asker) = answering(Box::new(|_| Some((Duration::ZERO, login("s3cret", true)))));
    let outcome = mount(&bus.address, "nas", &nas(), &prompt, Duration::from_secs(10));
    drop(prompt);
    let log = asker.join().unwrap();
    daemon.join().unwrap();
    assert!(matches!(outcome, Ok(Outcome::Mounted(_))), "{outcome:?}");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.prefix, b"/docs\0");
    let spec: Vec<(String, Vec<u8>)> = vec![("server".into(), b"nas\0".to_vec()), ("share".into(), b"share\0".to_vec()), ("type".into(), b"smb-share\0".to_vec()), ("user".into(), b"ann\0".to_vec())];
    assert_eq!(seen.spec, spec, "every value is a NUL-terminated byte string");
    assert_eq!(seen.replies, [vec![Arg::I(1), Arg::I(0), Arg::S("s3cret".into()), Arg::S("ann".into()), Arg::S(String::new()), Arg::I(0), Arg::I(1)]]);
    assert_eq!(log.asks.len(), 1);
    assert!(matches!(&log.asks[0].1, MountAsk::Login(l) if l.server == "nas" && l.user == "ann" && l.flags == ASK_PASSWORD | ASK_SAVING && l.message == "Password for ann on nas"));
    assert!(log.done.is_empty(), "an answered form is not withdrawn");
}

#[test]
fn cancelling_the_password_form_aborts_the_mount_without_an_error() {
    let Some(bus) = private_bus() else { return };
    let (seen, daemon) = fake_gvfs(&bus.address, Script::Password, "s3cret");
    let (prompt, asker) = answering(Box::new(|_| Some((Duration::ZERO, MountReply::Cancel))));
    let outcome = mount(&bus.address, "nas", &nas(), &prompt, Duration::from_secs(10));
    drop(prompt);
    asker.join().unwrap();
    daemon.join().unwrap();
    assert_eq!(outcome.unwrap(), Outcome::Cancelled);
    assert_eq!(seen.lock().unwrap().replies, [vec![Arg::I(1), Arg::I(1), Arg::S(String::new()), Arg::S(String::new()), Arg::S(String::new()), Arg::I(0), Arg::I(0)]]);
}

#[test]
fn a_form_dropped_without_an_answer_aborts_and_says_a_login_is_needed() {
    let Some(bus) = private_bus() else { return };
    let (seen, daemon) = fake_gvfs(&bus.address, Script::Password, "s3cret");
    let (prompt, asker) = answering(Box::new(|_| None));
    let outcome = mount(&bus.address, "nas", &nas(), &prompt, Duration::from_secs(10));
    drop(prompt);
    asker.join().unwrap();
    daemon.join().unwrap();
    assert_eq!(outcome.map_err(|e| e.kind()), Err(std::io::ErrorKind::PermissionDenied));
    assert_eq!(seen.lock().unwrap().replies[0][..2], [Arg::I(1), Arg::I(1)]);
}

#[test]
fn a_wrong_password_is_reported_as_the_server_error() {
    let Some(bus) = private_bus() else { return };
    let (_seen, daemon) = fake_gvfs(&bus.address, Script::Password, "s3cret");
    let (prompt, asker) = answering(Box::new(|_| Some((Duration::ZERO, login("guess", false)))));
    let outcome = mount(&bus.address, "nas", &nas(), &prompt, Duration::from_secs(10));
    drop(prompt);
    asker.join().unwrap();
    daemon.join().unwrap();
    assert_eq!(outcome.map_err(|e| e.to_string()), Err("Permission denied".to_owned()));
}

#[test]
fn the_wait_is_paused_while_the_password_form_is_open() {
    let Some(bus) = private_bus() else { return };
    let (_seen, daemon) = fake_gvfs(&bus.address, Script::Password, "s3cret");
    let (prompt, asker) = answering(Box::new(|_| Some((Duration::from_millis(1500), login("s3cret", false)))));
    let outcome = mount(&bus.address, "nas", &nas(), &prompt, Duration::from_millis(600));
    drop(prompt);
    asker.join().unwrap();
    daemon.join().unwrap();
    assert!(matches!(outcome, Ok(Outcome::Mounted(_))), "a slow typist is not cut off by the wait: {outcome:?}");
}

#[test]
fn a_server_that_never_answers_times_out_after_the_wait() {
    let Some(bus) = private_bus() else { return };
    let (_seen, daemon) = fake_gvfs(&bus.address, Script::Silent, "");
    let (prompt, asker) = answering(Box::new(|_| panic!("nothing is asked")));
    let started = Instant::now();
    let outcome = mount(&bus.address, "nas", &nas(), &prompt, Duration::from_millis(500));
    let took = started.elapsed();
    drop(prompt);
    asker.join().unwrap();
    daemon.join().unwrap();
    assert_eq!(outcome.map_err(|e| e.kind()), Err(std::io::ErrorKind::TimedOut));
    assert!(took >= Duration::from_millis(500) && took < Duration::from_secs(3), "{took:?}");
}

#[test]
fn without_gvfs_on_the_bus_the_mount_is_left_to_gio() {
    let (prompt, asker) = answering(Box::new(|_| panic!("nothing is asked")));
    assert_eq!(mount("unix:path=/nonexistent/flit-test-bus", "nas", &nas(), &prompt, Duration::from_secs(2)).unwrap(), Outcome::Unavailable);
    if let Some(bus) = private_bus() {
        assert_eq!(mount(&bus.address, "nas", &nas(), &prompt, Duration::from_secs(2)).unwrap(), Outcome::Unavailable, "a bus without org.gtk.vfs.Daemon");
    }
    drop(prompt);
    assert!(asker.join().unwrap().asks.is_empty());
}

#[test]
fn a_host_key_question_shows_the_choices_and_returns_the_picked_one() {
    let Some(bus) = private_bus() else { return };
    let (seen, daemon) = fake_gvfs(&bus.address, Script::Question, "");
    let (prompt, asker) = answering(Box::new(|_| Some((Duration::ZERO, MountReply::Choice(0)))));
    let outcome = mount(&bus.address, "host", &spec("sftp://host/").unwrap(), &prompt, Duration::from_secs(10));
    drop(prompt);
    let log = asker.join().unwrap();
    daemon.join().unwrap();
    assert!(matches!(outcome, Ok(Outcome::Mounted(_))), "{outcome:?}");
    assert_eq!(log.asks[0].1, MountAsk::Question { server: "host".into(), message: "The identity of host cannot be verified".into(), choices: vec!["Log In Anyway".into(), "Cancel Login".into()] });
    assert_eq!(seen.lock().unwrap().replies, [vec![Arg::I(1), Arg::I(0), Arg::I(0)]]);
}

#[test]
fn gvfs_giving_up_closes_the_open_form() {
    let Some(bus) = private_bus() else { return };
    let (seen, daemon) = fake_gvfs(&bus.address, Script::AbortWhileAsking, "");
    let (prompt, asker) = answering(Box::new(|_| Some((Duration::from_secs(4), MountReply::Choice(0)))));
    let outcome = mount(&bus.address, "nas", &nas(), &prompt, Duration::from_secs(10));
    drop(prompt);
    let log = asker.join().unwrap();
    daemon.join().unwrap();
    assert!(outcome.is_err(), "{outcome:?}");
    assert_eq!(log.done, [log.asks[0].0], "the form of that request is withdrawn");
    let replies = &seen.lock().unwrap().replies;
    assert!(replies.iter().any(|r| r.len() == 7 && r[..2] == [Arg::I(1), Arg::I(1)]), "the held request is answered as aborted: {replies:?}");
}

#[test]
fn the_password_reply_carries_no_secret_in_debug_output() {
    let reply = login("hunter2", true);
    assert!(!format!("{reply:?}").contains("hunter2"));
    let mut s = String::from("hunter2");
    wipe(&mut s);
    assert!(s.is_empty());
}

#[test]
#[ignore]
fn live_gvfs_sftp_login() {
    let uri = std::env::var("FLIT_GVFS_LIVE_URI").expect("FLIT_GVFS_LIVE_URI");
    let marker = std::env::var("FLIT_GVFS_LIVE_MARKER").expect("FLIT_GVFS_LIVE_MARKER");
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", std::env::var("FLIT_GVFS_LIVE_BUS").expect("FLIT_GVFS_LIVE_BUS"));
    std::env::set_var("XDG_RUNTIME_DIR", std::env::var("FLIT_GVFS_LIVE_RUNTIME").expect("FLIT_GVFS_LIVE_RUNTIME"));
    let mut secret = String::new();
    std::io::stdin().read_line(&mut secret).expect("the password on stdin");
    let len = secret.trim_end_matches('\n').len();
    secret.truncate(len);
    let shared = Arc::new(Mutex::new(std::mem::take(&mut secret)));
    let typed = shared.clone();
    let address = dbus::session_address().expect("a session bus");
    let target = spec(&uri).expect("a spec");
    let host = server(&uri);
    let question = |ask: &MountAsk| matches!(ask, MountAsk::Question { .. }).then(|| (Duration::ZERO, MountReply::Choice(0)));
    let run = |answer: Answerer, wait: Duration| {
        let (prompt, asker) = answering(answer);
        let outcome = mount(&address, &host, &target, &prompt, wait);
        drop(prompt);
        (outcome, asker.join().unwrap())
    };
    let logins = |log: &Log| log.asks.iter().filter(|(_, a)| matches!(a, MountAsk::Login(_))).count();
    let questions = |log: &Log| log.asks.iter().filter(|(_, a)| matches!(a, MountAsk::Question { .. })).count();

    let (outcome, log) = run(Box::new(move |a| question(a).or(Some((Duration::ZERO, MountReply::Cancel)))), Duration::from_secs(45));
    let flags = log.asks.iter().find_map(|(_, a)| if let MountAsk::Login(l) = a { Some(l.flags) } else { None });
    println!("live-cancel outcome={outcome:?} logins={} questions={} flags={flags:?}", logins(&log), questions(&log));
    assert_eq!(outcome.unwrap(), Outcome::Cancelled);

    let (outcome, log) = run(Box::new(move |a| question(a).or(None)), Duration::from_secs(45));
    println!("live-abandon kind={:?} logins={}", outcome.as_ref().map_err(|e| e.kind()), logins(&log));
    assert_eq!(outcome.map_err(|e| e.kind()), Err(std::io::ErrorKind::PermissionDenied));

    let mut tries = 0;
    let (outcome, log) = run(
        Box::new(move |a| {
            question(a).or_else(|| {
                tries += 1;
                Some((Duration::ZERO, if tries == 1 { login_as(a, "not-the-password") } else { MountReply::Cancel }))
            })
        }),
        Duration::from_secs(45),
    );
    println!("live-wrong-then-cancel ok={} logins={}", !matches!(outcome, Ok(Outcome::Mounted(_))), logins(&log));
    assert!(!matches!(outcome, Ok(Outcome::Mounted(_))));

    let started = Instant::now();
    let (outcome, log) = run(Box::new(move |a| question(a).or_else(|| Some((Duration::from_secs(5), login_as(a, &typed.lock().unwrap()))))), Duration::from_secs(3));
    println!("live-slow-answer mounted={} logins={} questions={} took-over-wait={}", matches!(outcome, Ok(Outcome::Mounted(_))), logins(&log), questions(&log), started.elapsed() > Duration::from_secs(3));
    assert!(matches!(outcome, Ok(Outcome::Mounted(_))), "{outcome:?}");
    assert!(log.done.is_empty());

    let point = crate::places::local_path_within(&uri, Duration::from_secs(10)).expect("a local path");
    let runtime = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    let text = std::fs::read_to_string(point.join(marker.trim_start_matches('/'))).expect("the marker through FUSE");
    println!("live-local-path under-runtime-gvfs={} marker-read={}", point.starts_with(std::path::Path::new(&runtime).join("gvfs")), text.trim() == "flit-l37-live-marker");
    assert!(point.starts_with(std::path::Path::new(&runtime).join("gvfs")));
    assert_eq!(text.trim(), "flit-l37-live-marker");

    let (outcome, log) = run(Box::new(|a| panic!("asked again: {a:?}")), Duration::from_secs(45));
    println!("live-already-mounted mounted={} asks={}", matches!(outcome, Ok(Outcome::Mounted(_))), log.asks.len());
    assert!(matches!(outcome, Ok(Outcome::Mounted(_))), "{outcome:?}");

    let (tx, _rx) = crossbeam_channel::unbounded();
    let via = connect(&uri, &MountPrompt::new(9, tx, Arc::new(|| {}))).expect("connect");
    println!("live-connect same-path={}", via.as_deref() == Some(point.as_path()));
    assert_eq!(via.as_deref(), Some(point.as_path()));
    wipe(&mut shared.lock().unwrap());
    println!("live-ok");
}
