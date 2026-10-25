use super::*;
use crossbeam_channel::{Receiver, Sender};
use flit_core::attr;
use flit_proto::transport::{encode, handshake, read_frame};
use flit_search::index::{IndexRecord, IndexSink, VolumeIndex};
use flit_search::proto::{Sort, SortKey};
use std::io::{self, Write};
use std::time::Instant;

struct MemPipe {
    rx: Receiver<Vec<u8>>,
    tx: Sender<Vec<u8>>,
    pending: Vec<u8>,
    timeout: Option<Duration>,
}

impl Read for MemPipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            let got = match self.timeout {
                Some(t) => self.rx.recv_timeout(t).map_err(|e| e.is_timeout()),
                None => self.rx.recv().map_err(|_| false),
            };
            match got {
                Ok(b) => self.pending = b,
                Err(true) => return Err(io::ErrorKind::TimedOut.into()),
                Err(false) => return Ok(0),
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

impl Write for MemPipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tx.send(buf.to_vec()).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Duplex for MemPipe {
    fn try_clone(&self) -> io::Result<Self> {
        Ok(Self { rx: self.rx.clone(), tx: self.tx.clone(), pending: Vec::new(), timeout: self.timeout })
    }
    fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }
}

fn pair() -> (MemPipe, MemPipe) {
    let (a_tx, a_rx) = unbounded();
    let (b_tx, b_rx) = unbounded();
    (MemPipe { rx: a_rx, tx: b_tx, pending: Vec::new(), timeout: None }, MemPipe { rx: b_rx, tx: a_tx, pending: Vec::new(), timeout: None })
}

fn shared() -> Arc<SharedIndex> {
    let s = SharedIndex::new();
    let mut v = VolumeIndex::new("C:", br"C:\", b'\\');
    v.upsert(&IndexRecord { id: 5, parent: 5, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    for k in 0..10u64 {
        let name = format!("report{k}.txt");
        v.upsert(&IndexRecord { id: 100 + k, parent: 5, name: name.as_bytes(), size: k, modified: 0, attrs: 0 });
    }
    v.commit();
    let mut e = s.write();
    e.volumes.push(v);
    e.version = 1;
    drop(e);
    s
}

fn start() -> (MemPipe, std::thread::JoinHandle<Result<(), ProtoError>>) {
    let (client, server) = pair();
    let sh = shared();
    (client, std::thread::spawn(move || serve(server, sh, 0)))
}

const SORT: Sort = Sort { key: SortKey::Name, descending: false };

#[test]
fn query_and_rows_round_trip() {
    let (mut c, server) = start();
    handshake(&mut c, &Hello::new(Protocol::Index, VERSION)).unwrap();
    write_frame(&mut c, &Message::Query { gen: 1, text: "report ext:txt".into(), sort: SORT }).unwrap();
    let mut buf = Vec::new();
    let mut next = || read_frame::<_, Message>(&mut c, &mut buf).unwrap();
    let mut got = next();
    while matches!(got, Message::IndexStatus { .. }) {
        got = next();
    }
    assert_eq!(got, Message::QueryDone { gen: 1, total: 10 });
    assert!(matches!(next(), Message::Rows { gen: 1, from: 0, entries, .. } if entries.len() == 10));
    write_frame(&mut c, &Message::GetRows { gen: 1, from: 8, to: 20 }).unwrap();
    let mut got = read_frame::<_, Message>(&mut c, &mut buf).unwrap();
    while matches!(got, Message::IndexStatus { .. }) {
        got = read_frame::<_, Message>(&mut c, &mut buf).unwrap();
    }
    let Message::Rows { from: 8, entries, dirs, .. } = got else { panic!("{got:?}") };
    assert_eq!(entries.len(), 2);
    assert_eq!(dirs[1], br"C:\".to_vec());
    drop(c);
    assert!(server.join().unwrap().is_ok());
}

#[test]
fn version_mismatch_is_reported_to_both_sides() {
    let (mut c, server) = start();
    let err = handshake(&mut c, &Hello::new(Protocol::Index, VERSION + 1)).unwrap_err();
    assert!(matches!(err, ProtoError::VersionMismatch { theirs, .. } if theirs == VERSION));
    assert!(matches!(server.join().unwrap(), Err(ProtoError::VersionMismatch { .. })));
}

fn after_hello(bytes: &[u8]) -> Result<(), ProtoError> {
    let (mut c, server) = start();
    handshake(&mut c, &Hello::new(Protocol::Index, VERSION)).unwrap();
    c.write_all(bytes).unwrap();
    let r = server.join().unwrap();
    drop(c);
    r
}

#[test]
fn hostile_frames_drop_the_connection() {
    assert!(matches!(after_hello(&((MAX_REQUEST + 1) as u32).to_le_bytes()), Err(ProtoError::TooLarge(_))));
    assert!(matches!(after_hello(&[3, 0, 0, 0, 0xFF, 0xFF, 0xFF]), Err(ProtoError::Decode(_))));
    let mut bad = Vec::new();
    encode(&Message::QueryDone { gen: 1, total: 5 }, &mut bad).unwrap();
    assert!(matches!(after_hello(&bad), Err(ProtoError::Invalid(_))));
    let started = Instant::now();
    assert!(matches!(after_hello(&[200, 0, 0, 0, 1]), Err(ProtoError::Io(e)) if e.kind() == io::ErrorKind::TimedOut), "a stalled frame times out");
    assert!(started.elapsed() < READ_TIMEOUT * 3);
}

#[test]
fn silent_and_foreign_clients_are_rejected() {
    let (c, server) = start();
    let started = Instant::now();
    assert!(matches!(server.join().unwrap(), Err(ProtoError::Io(_))));
    assert!(started.elapsed() >= READ_TIMEOUT - Duration::from_millis(100));
    drop(c);
    let (mut c, server) = start();
    c.write_all(b"\x05\x00\x00\x00hello").unwrap();
    assert!(matches!(server.join().unwrap(), Err(ProtoError::NotFlit)));
}
