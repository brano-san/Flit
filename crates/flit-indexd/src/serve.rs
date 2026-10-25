use crossbeam_channel::{unbounded, RecvTimeoutError};
use flit_platform::Duplex;
use flit_proto::transport::{read_frame_limited, read_hello, write_frame, Hello, Protocol};
use flit_proto::ProtoError;
use flit_search::proto::{Message, VERSION};
use flit_search::session::{Session, SharedIndex};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const MAX_CLIENTS: usize = 16;
pub const MAX_REQUEST: usize = 64 << 10;
pub const READ_TIMEOUT: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(250);

pub fn serve<C: Duplex>(mut conn: C, shared: Arc<SharedIndex>, utc_offset_min: i32) -> Result<(), ProtoError> {
    conn.set_read_timeout(Some(READ_TIMEOUT));
    let ours = Hello::new(Protocol::Index, VERSION);
    let theirs = read_hello(&mut conn, &ours);
    write_frame(&mut conn, &ours)?;
    theirs?;
    let latest = Arc::new(AtomicU64::new(0));
    let (tx, rx) = unbounded::<Message>();
    let mut writer = conn.try_clone()?;
    let worker_latest = latest.clone();
    let worker = std::thread::Builder::new().name("flit-indexd-session".into()).spawn(move || {
        let mut session = Session::new(shared, utc_offset_min);
        let mut write = |m: Message| {
            let _ = write_frame(&mut writer, &m);
        };
        loop {
            match rx.recv_timeout(TICK) {
                Ok(msg) => session.handle(msg, &worker_latest, &mut write),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            session.tick(&worker_latest, &mut write);
        }
    })?;
    let mut buf = Vec::new();
    let result = loop {
        conn.set_read_timeout(None);
        let mut first = [0u8; 1];
        match conn.read(&mut first) {
            Ok(1) => {}
            Ok(_) => break Ok(()),
            Err(e) => break Err(e.into()),
        }
        conn.set_read_timeout(Some(READ_TIMEOUT));
        let msg = match read_frame_limited::<_, Message>(&mut (&first[..]).chain(&mut conn), &mut buf, MAX_REQUEST) {
            Ok(m) => m,
            Err(e) => break Err(e),
        };
        match &msg {
            Message::Query { gen, .. } => latest.store(*gen, Ordering::Release),
            Message::GetRows { .. } => {}
            _ => break Err(ProtoError::Invalid("clients may only send Query and GetRows")),
        }
        if tx.send(msg).is_err() {
            break Ok(());
        }
    };
    drop(tx);
    let _ = worker.join();
    result
}

#[cfg(test)]
#[path = "../test/serve.rs"]
mod tests;
