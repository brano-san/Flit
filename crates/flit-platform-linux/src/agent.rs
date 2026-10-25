use crate::fs::{dir_sizes, fs_error, host, scan, stat_path};
use crate::inotify::{Notice, Notify};
use crate::trash::Trash;
use crossbeam_channel::{unbounded, Sender};
use flit_core::provider::FileOp;
use flit_core::{FsError, FsErrorKind, FsPath};
use flit_proto::listing::{check_name, EntryBatch};
use flit_proto::provider::{Event, ReqId, Request, VERSION};
use flit_proto::transport::{encode, read_frame, read_hello, Hello, ProtoError, Protocol};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{self, BufWriter, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BATCH_EVERY: Duration = Duration::from_millis(16);
const BATCH_MAX: usize = 4096;
const UNAVAILABLE: FsError = FsError { kind: FsErrorKind::Unavailable, code: 0 };

pub fn hello(build: String) -> Hello {
    Hello { build, ..Hello::new(Protocol::Provider, VERSION) }
}

#[derive(Clone)]
struct Ctx {
    events: Sender<Event>,
    cancelled: Arc<Mutex<HashSet<ReqId>>>,
    trash: Arc<Option<Trash>>,
}

impl Ctx {
    fn send(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    fn is_cancelled(&self, id: ReqId) -> bool {
        self.cancelled.lock().unwrap_or_else(|e| e.into_inner()).contains(&id)
    }

    fn finish(&self, id: ReqId) -> bool {
        !self.cancelled.lock().unwrap_or_else(|e| e.into_inner()).remove(&id)
    }

    fn cancel(&self, id: ReqId) {
        self.cancelled.lock().unwrap_or_else(|e| e.into_inner()).insert(id);
    }
}

fn write_loop(output: impl Write, ours: Hello, events: crossbeam_channel::Receiver<Event>) {
    let mut w = BufWriter::with_capacity(1 << 20, output);
    let mut buf = Vec::new();
    if encode(&ours, &mut buf).is_err() {
        return;
    }
    let _ = w.write_all(&buf).and_then(|()| w.flush());
    while let Ok(ev) = events.recv() {
        buf.clear();
        for ev in std::iter::once(ev).chain(events.try_iter()) {
            if let Err(e) = encode(&ev, &mut buf) {
                eprintln!("flit-wsl-agent: dropped an event: {e}");
            }
        }
        if w.write_all(&buf).and_then(|()| w.flush()).is_err() {
            return;
        }
    }
}

fn list(ctx: &Ctx, id: ReqId, path: &FsPath) {
    let Some(dir) = host(path) else { return ctx.send(Event::ListDone { id, error: Some(UNAVAILABLE) }) };
    let mut batch = EntryBatch::default();
    let mut last = Instant::now();
    let mut stopped = false;
    let result = scan(dir, |name, meta| {
        batch.push(name, meta);
        if batch.len() >= BATCH_MAX || (batch.len() % 64 == 0 && last.elapsed() >= BATCH_EVERY) {
            if ctx.is_cancelled(id) {
                stopped = true;
                return false;
            }
            ctx.send(Event::Entries { id, batch: std::mem::take(&mut batch) });
            last = Instant::now();
        }
        true
    });
    if ctx.finish(id) && !stopped {
        if !batch.is_empty() {
            ctx.send(Event::Entries { id, batch });
        }
        ctx.send(Event::ListDone { id, error: result.err().map(|e| fs_error(&e)) });
    }
}

fn sizes(ctx: &Ctx, id: ReqId, dir: &FsPath, items: Vec<(u32, Vec<u8>)>) {
    let Some(root) = host(dir) else { return items.iter().for_each(|(idx, _)| ctx.send(Event::DirSize { id, idx: *idx, size: 0 })) };
    let (valid, invalid): (Vec<_>, Vec<_>) = items.into_iter().partition(|(_, name)| check_name(name, false).is_ok());
    for (idx, name) in invalid {
        eprintln!("flit-wsl-agent: refused folder size of {:?}", String::from_utf8_lossy(&name));
        ctx.send(Event::DirSize { id, idx, size: 0 });
    }
    let valid: Vec<(u32, PathBuf)> = valid.into_iter().map(|(i, n)| (i, PathBuf::from(OsStr::from_bytes(&n)))).collect();
    let keep_going = || !ctx.is_cancelled(id);
    dir_sizes(root, &valid, &keep_going, &|idx, size| ctx.send(Event::DirSize { id, idx, size }));
    ctx.finish(id);
}

fn trash_all(trash: Option<&Trash>, paths: &[FsPath]) -> Option<String> {
    let Some(trash) = trash else { return Some("The Trash folder is unknown: neither XDG_DATA_HOME nor HOME is set".into()) };
    let failures: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let path = host(p).ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL)).and_then(|h| trash.trash(h).map(drop));
            path.err().map(|e| format!("Cannot move {} to the Trash: {e}", p.display()))
        })
        .collect();
    match failures.len() {
        0 => None,
        1 => failures.into_iter().next(),
        n => Some(format!("{n} items failed. {}", failures[0])),
    }
}

fn op(ctx: &Ctx, id: ReqId, op: &FileOp) {
    let error = match op {
        FileOp::Trash(paths) => trash_all(ctx.trash.as_ref().as_ref(), paths),
        _ => Some("The Linux agent does not run this operation".into()),
    };
    ctx.send(Event::OpDone { id, error });
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(body) {
        eprintln!("flit-wsl-agent: cannot start {name}: {e}");
    }
}

fn watch(notify: Option<&Notify<ReqId>>, id: ReqId, path: &FsPath) {
    let result = match (notify, host(path)) {
        (Some(n), Some(dir)) => n.watch(id, dir),
        (None, _) => Err(io::Error::other("inotify is not available")),
        (_, None) => Err(io::Error::from_raw_os_error(libc::EINVAL)),
    };
    match result {
        Err(e) if e.raw_os_error() == Some(libc::ENOSPC) => eprintln!("flit-wsl-agent: cannot watch {}: fs.inotify.max_user_watches is exhausted", path.display()),
        Err(e) => eprintln!("flit-wsl-agent: cannot watch {}: {e}", path.display()),
        Ok(()) => {}
    }
}

pub fn serve(mut input: impl Read, output: impl Write + Send + 'static, build: String, trash: Option<Trash>) -> Result<(), ProtoError> {
    let (events, rx) = unbounded();
    let ours = hello(build);
    let theirs = ours.clone();
    spawn("flit-agent-out", move || write_loop(output, theirs, rx));
    read_hello(&mut input, &ours)?;
    let ctx = Ctx { events, cancelled: Arc::default(), trash: Arc::new(trash) };
    let sink = ctx.events.clone();
    let notify = Notify::new(move |id, notice| {
        let _ = sink.send(match notice {
            Notice::Changes(changes) => Event::Changes { id, changes },
            Notice::Rescan => Event::Rescan { id },
        });
    })
    .map_err(|e| eprintln!("flit-wsl-agent: inotify: {e}"))
    .ok();
    let mut buf = Vec::new();
    loop {
        let req: Request = match read_frame(&mut input, &mut buf) {
            Ok(r) => r,
            Err(ProtoError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let c = ctx.clone();
        match req {
            Request::Shutdown => return Ok(()),
            Request::Cancel { id } => {
                if !notify.as_ref().is_some_and(|n| n.unwatch(&id)) {
                    ctx.cancel(id);
                }
            }
            Request::Watch { id, path } => watch(notify.as_ref(), id, &path),
            Request::Stat { id, path } => {
                let result = host(&path).map_or(Err(UNAVAILABLE), |p| stat_path(p).map_err(|e| fs_error(&e)));
                ctx.send(Event::Stat { id, result });
            }
            Request::List { id, path } => spawn("flit-agent-list", move || list(&c, id, &path)),
            Request::DirSizes { id, dir, items } => spawn("flit-agent-sizes", move || sizes(&c, id, &dir, items)),
            Request::Op { id, op: o } => spawn("flit-agent-op", move || op(&c, id, &o)),
        }
    }
}

pub fn own_build() -> String {
    std::fs::read("/proc/self/exe").map(|b| flit_proto::build_id::build_id(&b)).unwrap_or_else(|e| {
        eprintln!("flit-wsl-agent: cannot read /proc/self/exe: {e}");
        String::new()
    })
}

#[cfg(test)]
#[path = "../test/agent.rs"]
mod tests;
