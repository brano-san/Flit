use super::*;
use crate::temp::Temp;
use flit_proto::transport::{handshake, read_frame, read_hello, write_frame, Hello, Protocol};
use flit_search::proto::{Message, Sort, SortKey, VERSION};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn hello() -> Hello {
    Hello::new(Protocol::Index, VERSION)
}

fn mode(p: &Path) -> u32 {
    std::fs::symlink_metadata(p).unwrap().mode() & 0o777
}

fn chmod(p: &Path, mode: u32) {
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn answering(listener: Bound, owner: u32, reached: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        serve_on(
            listener,
            owner,
            4,
            Arc::new(move |mut conn: UnixConn| {
                reached.store(true, Ordering::Release);
                conn.set_read_timeout(Some(Duration::from_secs(5)));
                if read_hello(&mut conn, &hello()).is_err() || write_frame(&mut conn, &hello()).is_err() {
                    return;
                }
                let mut buf = Vec::new();
                while let Ok(Message::Query { gen, .. }) = read_frame::<_, Message>(&mut conn, &mut buf) {
                    let _ = write_frame(&mut conn, &Message::QueryDone { gen, total: 7 });
                }
            }),
        )
    });
}

fn query(gen: u64) -> Message {
    Message::Query { gen, text: "report".into(), sort: Sort { key: SortKey::Name, descending: false } }
}

#[test]
fn a_client_and_the_service_talk_the_index_protocol_through_a_private_socket() {
    let t = Temp::new("sock-talk");
    let path = t.path().join(SOCKET);
    let listener = bind_at(&path, uid()).unwrap();
    assert_eq!(mode(path.parent().unwrap()), 0o700, "only the user may enter the socket folder");
    assert_eq!(mode(&path), 0o600, "only the user may connect");
    let reached = Arc::new(AtomicBool::new(false));
    answering(listener, uid(), reached.clone());
    let mut conn = connect_at(&path, uid(), CONNECT_TIMEOUT).unwrap();
    assert_eq!(conn.peer_uid().unwrap(), uid());
    conn.set_read_timeout(Some(Duration::from_secs(5)));
    handshake(&mut conn, &hello()).unwrap();
    for gen in [1, 2] {
        write_frame(&mut conn, &query(gen)).unwrap();
        assert_eq!(read_frame::<_, Message>(&mut conn, &mut Vec::new()).unwrap(), Message::QueryDone { gen, total: 7 });
    }
    assert!(reached.load(Ordering::Acquire));
}

#[test]
fn foreign_open_or_linked_sockets_are_not_used() {
    let t = Temp::new("sock-foreign");
    let path = t.path().join(SOCKET);
    let _listener = bind_at(&path, uid()).unwrap();
    assert!(connect_at(&path, uid().wrapping_add(1), CONNECT_TIMEOUT).is_err(), "a socket of another user is never used");
    chmod(&path, 0o666);
    assert!(connect_at(&path, uid(), CONNECT_TIMEOUT).is_err(), "a socket anyone may open is not trusted");
    chmod(&path, 0o600);
    chmod(path.parent().unwrap(), 0o755);
    assert!(connect_at(&path, uid(), CONNECT_TIMEOUT).is_err(), "a socket in a shared folder is not trusted");
    chmod(path.parent().unwrap(), 0o700);
    let link = path.parent().unwrap().join("link.sock");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(connect_at(&link, uid(), CONNECT_TIMEOUT).is_err(), "a link to a socket is not followed");
    assert!(connect_at(&path, uid(), CONNECT_TIMEOUT).is_ok(), "the private socket itself still works");
}

#[test]
fn a_missing_or_dead_service_fails_at_once() {
    let t = Temp::new("sock-dead");
    let path = t.path().join(SOCKET);
    let started = Instant::now();
    assert!(connect_at(&path, uid(), CONNECT_TIMEOUT).is_err());
    drop(bind_at(&path, uid()).unwrap());
    assert!(path.exists(), "a crashed service leaves its socket behind");
    assert!(connect_at(&path, uid(), CONNECT_TIMEOUT).is_err());
    assert!(started.elapsed() < CONNECT_TIMEOUT, "nobody listening is not a timeout");
}

#[test]
fn a_stale_socket_is_replaced_but_a_live_one_or_a_file_is_kept() {
    let t = Temp::new("sock-stale");
    let path = t.path().join(SOCKET);
    drop(bind_at(&path, uid()).unwrap());
    let live = bind_at(&path, uid()).unwrap();
    assert!(bind_at(&path, uid()).is_err(), "a second service does not steal the socket");
    drop(live);
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"x").unwrap();
    assert!(bind_at(&path, uid()).is_err(), "a file in the way is not deleted");
    assert_eq!(std::fs::read(&path).unwrap(), b"x");
}

#[test]
fn clients_of_another_user_are_dropped_unserved() {
    let t = Temp::new("sock-peer");
    let path = t.path().join(SOCKET);
    let reached = Arc::new(AtomicBool::new(false));
    answering(bind_at(&path, uid()).unwrap(), uid().wrapping_add(1), reached.clone());
    let mut conn = connect_at(&path, uid(), CONNECT_TIMEOUT).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5)));
    assert!(handshake(&mut conn, &hello()).is_err());
    assert!(!reached.load(Ordering::Acquire));
}

#[test]
fn a_silent_service_times_out_reads_instead_of_hanging() {
    let t = Temp::new("sock-silent");
    let path = t.path().join(SOCKET);
    let listener = bind_at(&path, uid()).unwrap();
    let mut conn = connect_at(&path, uid(), CONNECT_TIMEOUT).unwrap();
    let _accepted = listener.listener().accept().unwrap();
    conn.set_read_timeout(Some(Duration::from_millis(100)));
    let started = Instant::now();
    let e = conn.read(&mut [0u8; 1]).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn only_sockets_passed_to_this_process_are_taken_from_systemd() {
    assert_eq!(listen_fd(Some("42"), Some("1"), 42), Some(3));
    assert_eq!(listen_fd(Some("41"), Some("1"), 42), None, "sockets meant for the parent process");
    assert_eq!(listen_fd(Some("42"), Some("0"), 42), None);
    assert_eq!(listen_fd(None, Some("1"), 42), None);
    assert_eq!(listen_fd(Some("x"), Some("1"), 42), None);
}

#[test]
fn the_socket_unit_listens_where_the_service_and_flit_look() {
    let unit = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/flit-indexd.socket")).unwrap();
    let lines: Vec<&str> = unit.lines().map(str::trim).collect();
    assert!(lines.contains(&format!("ListenStream=%t/{SOCKET}").as_str()), "{unit}");
    assert!(lines.contains(&"SocketMode=0600") && lines.contains(&"DirectoryMode=0700"), "{unit}");
}

fn busy_listener(path: &Path) -> (OwnedFd, Vec<OwnedFd>) {
    std::fs::DirBuilder::new().mode(0o700).create(path.parent().unwrap()).unwrap();
    let (addr, len) = address(path).unwrap();
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    let server = unsafe { OwnedFd::from_raw_fd(raw) };
    assert_eq!(unsafe { libc::bind(server.as_raw_fd(), (&addr as *const libc::sockaddr_un).cast(), len) }, 0);
    assert_eq!(unsafe { libc::listen(server.as_raw_fd(), 0) }, 0);
    let mut queued = Vec::new();
    loop {
        let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0) };
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if unsafe { libc::connect(fd.as_raw_fd(), (&addr as *const libc::sockaddr_un).cast(), len) } != 0 {
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EAGAIN));
            return (server, queued);
        }
        queued.push(fd);
    }
}

#[test]
fn only_a_refused_socket_counts_as_left_behind() {
    assert!(stale(&Err(io::Error::from_raw_os_error(libc::ECONNREFUSED))));
    assert!(!stale(&Err(io::ErrorKind::TimedOut.into())), "a busy service is alive");
    assert!(!stale(&Err(io::Error::from_raw_os_error(libc::EACCES))));
}

#[test]
fn a_busy_service_keeps_its_socket() {
    let t = Temp::new("sock-busy");
    let path = t.path().join(SOCKET);
    let _busy = busy_listener(&path);
    let started = Instant::now();
    assert!(bind_at(&path, uid()).is_err(), "a second service does not take over a busy one");
    assert!(started.elapsed() >= CONNECT_TIMEOUT, "the busy service was given time to answer");
    assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_socket(), "the busy service's socket stays");
}

#[test]
fn a_service_that_is_still_starting_holds_off_a_second_one() {
    let t = Temp::new("sock-lock");
    let path = t.path().join(SOCKET);
    std::fs::DirBuilder::new().mode(0o700).create(path.parent().unwrap()).unwrap();
    let other = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(lock_path(&path)).unwrap();
    assert_eq!(unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
    assert!(bind_at(&path, uid()).is_err());
    assert!(!path.exists(), "nothing is bound while another start holds the lock");
    drop(other);
    assert!(bind_at(&path, uid()).is_ok());
}

#[test]
fn of_two_services_started_together_exactly_one_serves() {
    let t = Temp::new("sock-race");
    let path = t.path().join(SOCKET);
    for _ in 0..20 {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let runs: Vec<_> = (0..2)
            .map(|_| {
                let (path, barrier) = (path.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    bind_at(&path, uid())
                })
            })
            .collect();
        let bound: Vec<Bound> = runs.into_iter().filter_map(|r| r.join().unwrap().ok()).collect();
        assert_eq!(bound.len(), 1);
        assert!(connect_at(&path, uid(), CONNECT_TIMEOUT).is_ok(), "the socket on disk is the winner's");
    }
}
