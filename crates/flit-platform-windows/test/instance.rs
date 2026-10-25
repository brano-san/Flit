use super::*;
use flit_core::open::parse_args;
use std::sync::{Arc, Mutex};
use windows::Win32::System::Pipes::CreatePipe;

fn test_pipe(tag: &str) -> Vec<u16> {
    format!(r"\\.\pipe\flit-test-{tag}-{}", std::process::id()).encode_utf16().chain(Some(0)).collect()
}

fn start_server(pipe: &[u16]) -> crossbeam_channel::Receiver<Open> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let sink: OpenSink = Arc::new(Mutex::new(Some(tx)));
    let pipe = pipe.to_vec();
    std::thread::spawn(move || listen_loop(pipe, sink, Arc::new(|| {})));
    rx
}

#[test]
fn deadline_reader_times_out_then_reads_what_arrived() {
    let (mut r, mut w) = (HANDLE::default(), HANDLE::default());
    unsafe { CreatePipe(&mut r, &mut w, None, 0) }.unwrap();
    let (r, mut w) = unsafe { (File::from_raw_handle(r.0), File::from_raw_handle(w.0)) };
    let started = Instant::now();
    let mut byte = [0u8; 1];
    let e = Deadline { pipe: &r, until: Instant::now() + Duration::from_millis(50) }.read(&mut byte).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(1));
    w.write_all(b"abc").unwrap();
    let mut buf = [0u8; 8];
    let n = Deadline::new(&r).read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"abc");
    drop(w);
    assert!(Deadline::new(&r).read(&mut buf).is_err());
}

#[test]
fn forwards_open_to_the_listener() {
    let pipe = test_pipe("forward");
    let rx = start_server(&pipe);
    let open = parse_args(&[b"--new-window".to_vec(), b"C:\\Windows".to_vec()], None);
    let deadline = Instant::now() + Duration::from_secs(3);
    while forward(&pipe, &open).is_err() {
        assert!(Instant::now() < deadline, "listener never came up");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), open);
}

#[test]
fn silent_and_oversized_clients_are_dropped() {
    let pipe = test_pipe("silent");
    let rx = start_server(&pipe);
    let started = Instant::now();
    let silent: Vec<File> = (0..MAX_CLIENTS).map(|_| connect(&pipe).unwrap()).collect();
    let extra = unsafe { CreateFileW(PCWSTR(pipe.as_ptr()), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_NONE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) };
    assert_eq!(extra.unwrap_err().code(), ERROR_PIPE_BUSY.to_hresult(), "client cap");
    let mut server_hello = Vec::new();
    (&silent[0]).read_to_end(&mut server_hello).unwrap();
    let waited = started.elapsed();
    assert!(!server_hello.is_empty());
    assert!(waited >= READ_TIMEOUT - Duration::from_millis(200) && waited < READ_TIMEOUT * 3, "{waited:?}");
    drop(silent);
    let mut byte = [0u8; 1];
    let mut big = connect(&pipe).unwrap();
    let mut out = Vec::new();
    flit_proto::transport::encode(&Hello::new(Protocol::Instance, VERSION), &mut out).unwrap();
    out.extend(((MAX_FRAME + 1) as u32).to_le_bytes());
    big.write_all(&out).unwrap();
    let theirs = read_hello(&mut Deadline::new(&big), &Hello::new(Protocol::Instance, VERSION));
    assert!(theirs.is_ok());
    assert!(!matches!(big.read(&mut byte), Ok(1)));
    assert!(rx.try_recv().is_err());
}
