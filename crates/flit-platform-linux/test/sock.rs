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

fn answering(listener: UnixListener, owner: u32, reached: Arc<AtomicBool>) {
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
    let _accepted = listener.accept().unwrap();
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
