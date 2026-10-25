use super::*;

fn test_pipe(tag: &str) -> String {
    format!(r"\\.\pipe\flit-test-index-{tag}-{}", std::process::id())
}

fn echo_server(name: &str, max: usize) {
    let name = name.to_owned();
    std::thread::spawn(move || {
        let on_client: OnClient<PipeConn> = Arc::new(|mut c: PipeConn| {
            let mut buf = [0u8; 64];
            while let Ok(n) = c.read(&mut buf) {
                if n == 0 || c.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        });
        let _ = listen_at(&name, max, on_client);
    });
}

fn connect_retrying(name: &str) -> PipeConn {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match connect_to(name) {
            Ok(c) => return c,
            Err(e) => assert!(Instant::now() < deadline, "server never came up: {e}"),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn dacl_depends_on_who_runs_the_service() {
    assert_eq!(dacl_for(SYSTEM_SID), "D:P(A;;GA;;;SY)(A;;0x12008b;;;IU)");
    assert_eq!(dacl_for("S-1-5-21-1-2-3-1001"), "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)");
}

#[test]
fn round_trip_timeouts_and_concurrent_read_write() {
    let name = test_pipe("echo");
    echo_server(&name, 2);
    let mut c = connect_retrying(&name);
    assert_eq!(c.peer_sid(), user_sid().as_deref(), "console server runs as this user");
    c.write_all(b"hello").unwrap();
    let mut got = [0u8; 5];
    c.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"hello");
    c.set_read_timeout(Some(Duration::from_millis(50)));
    let started = Instant::now();
    assert_eq!(c.read(&mut got).unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut reader = c.try_clone().unwrap();
    reader.set_read_timeout(None);
    let pending = std::thread::spawn(move || {
        let mut b = [0u8; 3];
        reader.read_exact(&mut b).map(|()| b)
    });
    std::thread::sleep(Duration::from_millis(50));
    c.write_all(b"abc").unwrap();
    assert_eq!(&pending.join().unwrap().unwrap(), b"abc", "a blocked read does not block writes on the same pipe");
}

#[test]
fn client_cap_and_missing_service() {
    let name = test_pipe("cap");
    echo_server(&name, 1);
    let first = connect_retrying(&name);
    let started = Instant::now();
    assert!(connect_to(&name).is_err(), "only one client at a time");
    assert!(started.elapsed() < Duration::from_secs(4));
    drop(first);
    let deadline = Instant::now() + Duration::from_secs(5);
    while connect_to(&name).is_err() {
        assert!(Instant::now() < deadline, "slot was not freed");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(connect_to(&test_pipe("nobody")).is_err());
}
