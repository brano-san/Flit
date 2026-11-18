use flit_core::i18n::{Lang, S};
use flit_core::provider::{AccessChange, FileOp};
use flit_core::FsPath;
use flit_platform::{AgentLauncher, AgentProcess, Job, OpOutcome, PlatformError};
use flit_proto::provider::{check_performed, Event, ReqId, Request, VERSION};
use flit_proto::transport::{read_frame, read_hello, write_frame, Hello, ProtoError, Protocol};
use std::ffi::{CString, OsStr, OsString};
use std::io::{self, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const FLAG: &str = "--elevated-agent";
const OP: ReqId = 1;
const CANCEL_POLL: Duration = Duration::from_millis(50);
const PKEXEC_DISMISSED: i32 = 126;
const PKEXEC_REFUSED: i32 = 127;

fn spoken(arg: Option<&OsStr>) -> Option<Lang> {
    arg?.to_str().and_then(Lang::from_code)
}

fn helper_args(exe: &Path) -> [OsString; 3] {
    [exe.as_os_str().to_owned(), FLAG.into(), flit_core::i18n::lang().code().into()]
}

pub fn serve(lang: Option<&OsStr>) -> i32 {
    if let Some(lang) = spoken(lang) {
        flit_core::i18n::set_lang(lang);
    }
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
            let mut paused = false;
            while !finished.load(Ordering::Acquire) {
                if job.cancelled() {
                    let _ = send(&output, &Request::Cancel { id: OP });
                    return;
                }
                if job.ctl().is_paused() != paused {
                    paused = !paused;
                    let _ = send(&output, &Request::Pause { id: OP, paused });
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
                if let Err(e) = send(&output, &Request::Answer { id: OP, choice, policy: job.ctl().policy() }) {
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

fn unelevated(why: impl Into<String>) -> PlatformError {
    PlatformError::new(libc::EACCES as i64, why)
}

pub fn run(op: &FileOp, job: &Job) -> Result<OpOutcome, PlatformError> {
    let pkexec = crate::tools::require("pkexec").map_err(|e| unelevated(e.message))?;
    let exe = std::env::current_exe().map_err(|e| unelevated(format!("cannot find the Flit program: {}", flit_core::fmt::io_reason(&e))))?;
    let mut child = Command::new(pkexec)
        .args(helper_args(&exe))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| unelevated(format!("pkexec: {}", flit_core::fmt::io_reason(&e))))?;
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
        Some(PKEXEC_DISMISSED) => unelevated(S::ErrorsOpsExtraAdminDismissed.en()),
        Some(PKEXEC_REFUSED) => unelevated(flit_core::tr_en!(S::ErrorsOpsExtraAdminRefused, why = complaint.trim())),
        _ if complaint.trim().is_empty() => PlatformError::new(-1, flit_core::tr_en!(S::ErrorsOpsExtraAdminStopped, why = e)),
        _ => PlatformError::new(-1, flit_core::tr_en!(S::ErrorsOpsExtraAdminStopped, why = complaint.trim())),
    })
}

fn only_elevated(e: PlatformError) -> OpOutcome {
    let error = match e.code == libc::EACCES as i64 {
        true => PlatformError::new(e.code, flit_core::tr_en!(S::ErrorsOpsExtraNeedsAdmin, why = e.message)),
        false => e,
    };
    OpOutcome { items: Vec::new(), error: Some(error) }
}

fn retried(first: OpOutcome, again: Result<OpOutcome, PlatformError>) -> OpOutcome {
    match (again, first.error) {
        (Ok(more), _) => OpOutcome { items: first.items.into_iter().chain(more.items).collect(), error: more.error },
        (Err(e), Some(denied)) => OpOutcome { items: first.items, error: Some(PlatformError::new(denied.code, flit_core::tr_en!(S::ErrorsOpsExtraNotElevated, error = denied.message, why = e.message))) },
        (Err(e), None) => OpOutcome { items: first.items, error: Some(e) },
    }
}

pub fn file_op(op: &FileOp, job: &Job, plain: impl FnOnce() -> OpOutcome) -> OpOutcome {
    if needs_root(op) {
        return run(op, job).unwrap_or_else(only_elevated);
    }
    let first = plain();
    let Some(rest) = retry(op, &first) else { return first };
    let again = run(&rest, job);
    retried(first, again)
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
        return run(op, job).unwrap_or_else(only_elevated);
    }
    let first = plain();
    let Some(rest) = access_retry(op, &first) else { return first };
    let before = job.take_performed();
    let again = run(&rest, job);
    let outcome = retried(first, again);
    if let Some(all) = joined(before, job.take_performed()) {
        job.performed(all);
    }
    outcome
}

pub fn grant_args(uid: u32, dir: &Path) -> Vec<OsString> {
    vec!["-m".into(), format!("u:{uid}:rX").into(), "--".into(), dir.as_os_str().to_owned()]
}

fn grant_failure(status: Option<i32>, complaint: &str) -> PlatformError {
    match status {
        Some(PKEXEC_DISMISSED) => unelevated(S::ErrorsOpsExtraAdminDismissed.en()),
        Some(PKEXEC_REFUSED) => unelevated(flit_core::tr_en!(S::ErrorsOpsExtraAdminRefused, why = complaint.trim())),
        code => PlatformError::new(code.map_or(-1, i64::from), format!("setfacl: {}", complaint.trim())),
    }
}

pub fn grant_access(dir: &FsPath) -> OpOutcome {
    let granted = (|| {
        let path = flit_platform_linux::host(dir).ok_or_else(|| PlatformError::new(-1, "not a Linux path"))?;
        let pkexec = crate::tools::require("pkexec").map_err(|e| unelevated(e.message))?;
        let setfacl = crate::tools::require("setfacl")?;
        let out = Command::new(pkexec).arg(setfacl).args(grant_args(unsafe { libc::getuid() }, path)).stdin(Stdio::null()).output().map_err(|e| unelevated(format!("pkexec: {}", flit_core::fmt::io_reason(&e))))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(grant_failure(out.status.code(), &String::from_utf8_lossy(&out.stderr)))
        }
    })();
    match granted {
        Ok(()) => OpOutcome { items: vec![(dir.clone(), dir.clone())], error: None },
        Err(e) => OpOutcome { items: Vec::new(), error: Some(e) },
    }
}

const COMPLAINT_MAX: usize = 4096;

type Spawner = Box<dyn Fn() -> io::Result<Command> + Send>;

pub struct RootLauncher {
    command: Spawner,
}

impl RootLauncher {
    pub fn pkexec() -> Self {
        Self::with(root_agent_command)
    }

    pub fn with(command: impl Fn() -> io::Result<Command> + Send + 'static) -> Self {
        Self { command: Box::new(command) }
    }
}

pub fn agent_program(exe: &Path) -> io::Result<PathBuf> {
    let refused = |key: S| io::Error::new(io::ErrorKind::PermissionDenied, key.en());
    if !exe.is_absolute() || exe.as_os_str().as_bytes().ends_with(b" (deleted)") {
        return Err(refused(S::ErrorsEngineAdminProgram));
    }
    let meta = std::fs::metadata(exe).map_err(|_| refused(S::ErrorsEngineAdminProgram))?;
    if !meta.is_file() {
        return Err(refused(S::ErrorsEngineAdminProgram));
    }
    if std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o022 != 0 {
        return Err(refused(S::ErrorsEngineAdminUnsafe));
    }
    Ok(exe.to_path_buf())
}

fn root_agent_command() -> io::Result<Command> {
    let pkexec = crate::tools::require("pkexec").map_err(|e| io::Error::new(io::ErrorKind::NotFound, e.message))?;
    let exe = agent_program(&std::env::current_exe().map_err(|_| io::Error::new(io::ErrorKind::NotFound, S::ErrorsEngineAdminProgram.en()))?)?;
    let mut command = Command::new(pkexec);
    command.args(helper_args(&exe));
    Ok(command)
}

pub fn refusal(status: Option<i32>, complaint: &str) -> String {
    let complaint = complaint.trim();
    match status {
        Some(PKEXEC_DISMISSED) => S::ErrorsOpsExtraAdminDismissed.en().to_owned(),
        Some(PKEXEC_REFUSED) => flit_core::tr_en!(S::ErrorsOpsExtraAdminRefused, why = complaint),
        _ if complaint.is_empty() => S::ErrorsEngineAdminSilent.en().to_owned(),
        _ => flit_core::tr_en!(S::ErrorsOpsExtraAdminStopped, why = complaint),
    }
}

type Shared = Arc<Mutex<Option<Child>>>;

struct AgentOutput {
    inner: ChildStdout,
    child: Shared,
    complaint: Option<std::thread::JoinHandle<String>>,
}

impl Read for AgentOutput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 || buf.is_empty() {
            return Ok(n);
        }
        let status = self.child.lock().unwrap_or_else(|e| e.into_inner()).take().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
        let complaint = self.complaint.take().and_then(|h| h.join().ok()).unwrap_or_default();
        Err(io::Error::other(refusal(status, &complaint)))
    }
}

fn read_complaint(mut stderr: impl Read) -> String {
    let mut all = Vec::new();
    let mut buf = [0u8; 1024];
    while let Ok(n) = stderr.read(&mut buf) {
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n]);
        if all.len() > COMPLAINT_MAX {
            all.drain(..all.len() - COMPLAINT_MAX);
        }
    }
    String::from_utf8_lossy(&all).into_owned()
}

fn reap(child: &Shared, grace: Duration) {
    let Some(mut c) = child.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    let until = std::time::Instant::now() + grace;
    while std::time::Instant::now() < until {
        if !matches!(c.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(CANCEL_POLL);
    }
    let _ = c.kill();
    let _ = c.wait();
}

impl AgentLauncher for RootLauncher {
    fn build(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn launch(&self) -> io::Result<AgentProcess> {
        let mut child = (self.command)()?.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| io::Error::new(e.kind(), format!("pkexec: {}", flit_core::fmt::io_reason(&e))))?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("pkexec started without its pipes"));
        };
        let complaint = std::thread::Builder::new().name("flit-admin-stderr".into()).spawn(move || read_complaint(stderr)).ok();
        let child: Shared = Arc::new(Mutex::new(Some(child)));
        let stopper = child.clone();
        Ok(AgentProcess {
            input: Box::new(stdin),
            output: Box::new(AgentOutput { inner: stdout, child, complaint }),
            stop: Box::new(move |grace| {
                let _ = std::thread::Builder::new().name("flit-admin-stop".into()).spawn(move || reap(&stopper, grace));
            }),
        })
    }

    fn deliver(&self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[cfg(test)]
#[path = "../test/elevated.rs"]
mod tests;
