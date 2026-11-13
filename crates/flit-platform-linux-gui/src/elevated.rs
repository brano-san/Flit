use flit_core::provider::{AccessChange, FileOp};
use flit_core::FsPath;
use flit_platform::{Job, OpOutcome, PlatformError};
use flit_proto::provider::{check_performed, Event, ReqId, Request, VERSION};
use flit_proto::transport::{read_frame, read_hello, write_frame, Hello, ProtoError, Protocol};
use std::ffi::{CString, OsStr};
use std::io::{BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const FLAG: &str = "--elevated-agent";
const OP: ReqId = 1;
const CANCEL_POLL: Duration = Duration::from_millis(50);
const PKEXEC_DISMISSED: i32 = 126;
const PKEXEC_REFUSED: i32 = 127;

pub fn serve() -> i32 {
    match flit_platform_linux::agent::serve(std::io::stdin().lock(), std::io::stdout(), env!("CARGO_PKG_VERSION").into(), None) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("flit: elevated helper: {e}");
            1
        }
    }
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn elevatable(op: &FileOp) -> bool {
    matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Rename { .. } | FileOp::NewFolder { .. } | FileOp::NewFile { .. } | FileOp::Delete(_) | FileOp::Link { .. })
}

fn host(p: &FsPath) -> Option<&Path> {
    flit_platform_linux::host(p)
}

fn parent(p: &FsPath) -> Option<PathBuf> {
    host(p)?.parent().map(Path::to_path_buf)
}

fn written_dirs(op: &FileOp) -> Vec<PathBuf> {
    let own = |p: &FsPath| host(p).map(Path::to_path_buf);
    match op {
        FileOp::Copy { dest, .. } | FileOp::Link { dest, .. } => own(dest).into_iter().collect(),
        FileOp::Move { items, dest } => own(dest).into_iter().chain(items.iter().filter_map(parent)).collect(),
        FileOp::MoveTo(pairs) => pairs.iter().flat_map(|(from, to)| [parent(from), parent(to)]).flatten().collect(),
        FileOp::Rename { item, .. } => parent(item).into_iter().collect(),
        FileOp::NewFolder { dir, .. } | FileOp::NewFile { dir, .. } => own(dir).into_iter().collect(),
        FileOp::Delete(items) => items.iter().filter_map(parent).collect(),
        _ => Vec::new(),
    }
}

fn denied(dir: &Path) -> bool {
    let Some(existing) = dir.ancestors().find(|a| a.exists()) else { return false };
    let Ok(c) = CString::new(existing.as_os_str().as_bytes()) else { return false };
    let refused = unsafe { libc::faccessat(libc::AT_FDCWD, c.as_ptr(), libc::W_OK | libc::X_OK, libc::AT_EACCESS) } != 0;
    refused && matches!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EACCES | libc::EPERM))
}

pub fn needs_root(op: &FileOp) -> bool {
    !is_root() && elevatable(op) && written_dirs(op).iter().any(|d| denied(d))
}

pub fn retry(op: &FileOp, out: &OpOutcome) -> Option<FileOp> {
    if is_root() || !elevatable(op) || out.error.as_ref().is_none_or(|e| e.code != libc::EACCES as i64) {
        return None;
    }
    if out.items.is_empty() {
        return Some(op.clone());
    }
    match op {
        FileOp::Delete(items) => {
            let rest: Vec<FsPath> = items.iter().filter(|i| !out.items.iter().any(|(done, _)| done == *i)).cloned().collect();
            (!rest.is_empty()).then_some(FileOp::Delete(rest))
        }
        _ => None,
    }
}

fn send(w: &Mutex<impl Write>, req: &Request) -> Result<(), ProtoError> {
    let mut w = w.lock().unwrap_or_else(|e| e.into_inner());
    write_frame(&mut *w, req)?;
    w.flush()?;
    Ok(())
}

pub fn drive<R: Read, W: Write + Send + 'static>(mut input: R, output: W, op: &FileOp, job: &Job) -> Result<OpOutcome, ProtoError> {
    let output = Arc::new(Mutex::new(output));
    let ours = Hello::new(Protocol::Provider, VERSION);
    {
        let mut w = output.lock().unwrap_or_else(|e| e.into_inner());
        write_frame(&mut *w, &ours)?;
        w.flush()?;
    }
    read_hello(&mut input, &ours)?;
    send(&output, &Request::Op { id: OP, op: op.clone(), policy: job.ctl().policy() })?;
    let finished = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (finished, output, job) = (finished.clone(), output.clone(), job.clone());
        std::thread::Builder::new().name("flit-elevated-cancel".into()).spawn(move || {
            while !finished.load(Ordering::Acquire) {
                if job.cancelled() {
                    let _ = send(&output, &Request::Cancel { id: OP });
                    return;
                }
                std::thread::sleep(CANCEL_POLL);
            }
        })
    };
    let mut buf = Vec::new();
    let result = loop {
        let ev: Event = match read_frame(&mut input, &mut buf) {
            Ok(ev) => ev,
            Err(e) => break Err(e),
        };
        match ev {
            Event::OpProgress { progress, .. } => job.update(|p| *p = progress),
            Event::OpConflict { conflict, .. } => {
                let choice = job.resolve(conflict);
                if let Err(e) = send(&output, &Request::Answer { id: OP, choice, all: false }) {
                    break Err(e);
                }
            }
            Event::OpFailure { failure, .. } => {
                let choice = job.fail(failure);
                if let Err(e) = send(&output, &Request::Recover { id: OP, choice, all: job.ctl().skips_failures() || !job.asks_failures() }) {
                    break Err(e);
                }
            }
            Event::OpDone { error, items, .. } => break Ok(OpOutcome { items, error: error.map(|m| PlatformError::new(-1, m)) }),
            Event::OpPerformed { op: done, .. } => {
                if let Err(e) = check_performed(op, &done) {
                    break Err(e);
                }
                job.performed(done);
            }
            _ => {}
        }
    };
    finished.store(true, Ordering::Release);
    if let Ok(w) = watcher {
        let _ = w.join();
    }
    let _ = send(&output, &Request::Shutdown);
    result
}

pub fn run(op: &FileOp, job: &Job) -> Result<OpOutcome, PlatformError> {
    let pkexec = crate::shell::on_path(OsStr::new("pkexec")).ok_or_else(|| PlatformError::new(libc::EACCES as i64, "This needs administrator rights, and pkexec (polkit) is not installed to ask for them"))?;
    let exe = std::env::current_exe().map_err(|e| PlatformError::new(libc::EACCES as i64, format!("cannot find the Flit program to run with administrator rights: {e}")))?;
    let mut child = Command::new(pkexec)
        .arg(&exe)
        .arg(FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("pkexec: {e}")))?;
    let (Some(stdin), Some(stdout), Some(mut stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        return Err(PlatformError::new(-1, "pkexec started without its pipes"));
    };
    let complaint = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let outcome = drive(BufReader::new(stdout), stdin, op, job);
    let status = child.wait().ok().and_then(|s| s.code());
    let complaint = complaint.join().unwrap_or_default();
    outcome.map_err(|e| match status {
        Some(PKEXEC_DISMISSED) => PlatformError::new(libc::EACCES as i64, "Administrator rights were not granted"),
        Some(PKEXEC_REFUSED) => PlatformError::new(libc::EACCES as i64, format!("Administrator rights were refused: {}", complaint.trim())),
        _ if complaint.trim().is_empty() => PlatformError::new(-1, format!("The elevated helper stopped: {e}")),
        _ => PlatformError::new(-1, format!("The elevated helper stopped: {}", complaint.trim())),
    })
}

pub fn file_op(op: &FileOp, job: &Job, plain: impl FnOnce() -> OpOutcome) -> OpOutcome {
    if needs_root(op) {
        return run(op, job).unwrap_or_else(|e| OpOutcome { items: Vec::new(), error: Some(e) });
    }
    let first = plain();
    let Some(rest) = retry(op, &first) else { return first };
    match run(&rest, job) {
        Ok(more) => OpOutcome { items: first.items.into_iter().chain(more.items).collect(), error: more.error },
        Err(_) => first,
    }
}

fn member(gid: u32) -> bool {
    if unsafe { libc::getegid() } == gid {
        return true;
    }
    let n = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    let mut groups = vec![0 as libc::gid_t; n.max(0) as usize];
    let n = unsafe { libc::getgroups(n.max(0), groups.as_mut_ptr()) };
    groups.truncate(n.max(0) as usize);
    groups.contains(&gid)
}

fn owners(p: &FsPath) -> Option<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(host(p)?).ok()?;
    Some((m.uid(), m.gid()))
}

pub fn access_needs_root(op: &FileOp) -> bool {
    if is_root() {
        return false;
    }
    let me = unsafe { libc::geteuid() };
    let changes = |p: &FsPath, uid: Option<u32>, gid: Option<u32>| {
        owners(p).is_some_and(|(u, g)| u != me || uid.is_some_and(|uid| uid != u) || gid.is_some_and(|gid| gid != g && !member(gid)))
    };
    match op {
        FileOp::SetAccess { items, edit } => items.iter().any(|i| changes(i, edit.owner, edit.group)),
        FileOp::Access(list) => list.iter().any(|c| changes(&c.item, Some(c.to.uid), Some(c.to.gid))),
        _ => false,
    }
}

pub fn access_retry(op: &FileOp, out: &OpOutcome) -> Option<FileOp> {
    if is_root() || out.error.as_ref().is_none_or(|e| e.code != libc::EACCES as i64) {
        return None;
    }
    match op {
        FileOp::SetAccess { .. } => Some(op.clone()),
        FileOp::Access(list) => {
            let rest: Vec<AccessChange> = list.iter().filter(|c| !out.items.iter().any(|(done, _)| done == &c.item)).cloned().collect();
            (!rest.is_empty()).then_some(FileOp::Access(rest))
        }
        _ => None,
    }
}

fn joined(first: Option<FileOp>, then: Option<FileOp>) -> Option<FileOp> {
    match (first, then) {
        (Some(FileOp::Access(mut a)), Some(FileOp::Access(b))) => {
            a.extend(b);
            Some(FileOp::Access(a))
        }
        (first, then) => then.or(first),
    }
}

pub fn access_op(op: &FileOp, job: &Job, plain: impl FnOnce() -> OpOutcome) -> OpOutcome {
    if access_needs_root(op) {
        return run(op, job).unwrap_or_else(|e| OpOutcome { items: Vec::new(), error: Some(e) });
    }
    let first = plain();
    let Some(rest) = access_retry(op, &first) else { return first };
    let before = job.take_performed();
    let outcome = match run(&rest, job) {
        Ok(more) => OpOutcome { items: first.items.into_iter().chain(more.items).collect(), error: more.error },
        Err(_) => first,
    };
    if let Some(all) = joined(before, job.take_performed()) {
        job.performed(all);
    }
    outcome
}

#[cfg(test)]
#[path = "../test/elevated.rs"]
mod tests;
