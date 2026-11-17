use flit_core::i18n::S;
use flit_core::provider::{DroppedFile, Stage};
use flit_core::FsPath;
use flit_platform::{Job, OpOutcome, PlatformError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, PropMode, Window};
use x11rb::xcb_ffi::XCBConnection;

pub const WAIT: Duration = Duration::from_secs(120);
const TICK: Duration = Duration::from_millis(100);
const MAX_NAME: usize = 255;
const NAME_WORDS: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Saved,
    Declined,
    Failed,
}

pub fn reply(bytes: &[u8]) -> Reply {
    match bytes.first() {
        Some(b'S') => Reply::Saved,
        Some(b'F') => Reply::Declined,
        _ => Reply::Failed,
    }
}

pub fn suggested_name(raw: &[u8]) -> Option<Vec<u8>> {
    let leaf = raw.split(|&b| b == b'/').rfind(|part| !part.is_empty())?;
    let kept: Vec<u8> = leaf.iter().copied().filter(|b| !b.is_ascii_control()).collect();
    let start = kept.iter().position(|b| !b.is_ascii_whitespace())?;
    let end = kept.iter().rposition(|b| !b.is_ascii_whitespace())? + 1;
    let mut name = kept[start..end].to_vec();
    if name.len() > MAX_NAME {
        let mut cut = MAX_NAME;
        while cut > 0 && name[cut] & 0xc0 == 0x80 {
            cut -= 1;
        }
        name.truncate(cut);
    }
    (flit_proto::listing::check_name(&name, false).is_ok() && name != b"." && name != b"..").then_some(name)
}

pub fn file_uri(path: &Path) -> Vec<u8> {
    format!("file://{}", flit_platform_linux::percent_encode(path.as_os_str().as_encoded_bytes())).into_bytes()
}

pub trait Link {
    fn direct_save(&self) -> Atom;
    fn owner(&self) -> Option<Window>;
    fn read_name(&self, window: Window) -> Option<Vec<u8>>;
    fn write_uri(&self, window: Window, uri: &[u8]) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub window: Window,
    pub name: Vec<u8>,
    pub atom: Atom,
}

pub fn probe(link: &dyn Link, offered: &[Atom]) -> Option<Offer> {
    let atom = link.direct_save();
    if atom == x11rb::NONE || !offered.contains(&atom) {
        return None;
    }
    let window = link.owner().filter(|&w| w != x11rb::NONE)?;
    let name = suggested_name(&link.read_name(window)?)?;
    Some(Offer { window, name, atom })
}

pub struct X11 {
    conn: XCBConnection,
    selection: Atom,
    direct_save: Atom,
    text: Atom,
}

impl X11 {
    pub fn connect() -> Option<Self> {
        let (conn, _) = XCBConnection::connect(None).ok()?;
        let atom = |name: &[u8]| conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom);
        let (selection, direct_save, text) = (atom(b"XdndSelection")?, atom(b"XdndDirectSave0")?, atom(b"text/plain")?);
        Some(Self { conn, selection, direct_save, text })
    }
}

impl Link for X11 {
    fn direct_save(&self) -> Atom {
        self.direct_save
    }

    fn owner(&self) -> Option<Window> {
        Some(self.conn.get_selection_owner(self.selection).ok()?.reply().ok()?.owner)
    }

    fn read_name(&self, window: Window) -> Option<Vec<u8>> {
        let reply = self.conn.get_property(false, window, self.direct_save, AtomEnum::ANY, 0, NAME_WORDS).ok()?.reply().ok()?;
        (reply.format == 8 && !reply.value.is_empty()).then_some(reply.value)
    }

    fn write_uri(&self, window: Window, uri: &[u8]) -> bool {
        let Ok(len) = u32::try_from(uri.len()) else { return false };
        let written = self.conn.change_property(PropMode::REPLACE, window, self.direct_save, self.text, 8, len, uri).is_ok();
        written && self.conn.get_input_focus().ok().and_then(|c| c.reply().ok()).is_some()
    }
}

pub fn x11() -> Option<&'static X11> {
    static LINK: OnceLock<Option<X11>> = OnceLock::new();
    LINK.get_or_init(X11::connect).as_ref()
}

pub struct Staging {
    pub dir: PathBuf,
    pub file: PathBuf,
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

struct Promise {
    data: Weak<DroppedFile>,
    replies: Receiver<Reply>,
    staging: Staging,
    fallback: Option<DroppedFile>,
}

static PROMISES: Mutex<Vec<Promise>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(0);

fn promises() -> std::sync::MutexGuard<'static, Vec<Promise>> {
    PROMISES.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Pending {
    pub data: Arc<DroppedFile>,
    pub sender: Sender<Reply>,
    pub uri: Vec<u8>,
    pub window: Window,
}

pub fn prepare(offer: &Offer, fallback: Option<DroppedFile>, root: &Path) -> std::io::Result<Pending> {
    use std::os::unix::ffi::OsStrExt;
    let dir = crate::dropped::private_dir(root, "flit-save", NEXT.fetch_add(1, Ordering::Relaxed), || false)?;
    let staging = Staging { file: dir.join(std::ffi::OsStr::from_bytes(&offer.name)), dir };
    let uri = file_uri(&staging.file);
    let data = Arc::new(DroppedFile { name: offer.name.clone(), bytes: Vec::new() });
    let (sender, replies) = std::sync::mpsc::channel();
    let mut all = promises();
    all.retain(|p| p.data.strong_count() > 0);
    all.push(Promise { data: Arc::downgrade(&data), replies, staging, fallback });
    Ok(Pending { data, sender, uri, window: offer.window })
}

fn take(data: &DroppedFile) -> Option<Promise> {
    let mut all = promises();
    let at = all.iter().position(|p| std::ptr::eq(p.data.as_ptr(), data))?;
    Some(all.swap_remove(at))
}

pub fn forget(data: &Arc<DroppedFile>) {
    drop(take(data));
}

#[derive(Debug, PartialEq, Eq)]
pub enum Waited {
    Answered(Reply),
    Silent,
    Cancelled,
}

pub fn wait(replies: &Receiver<Reply>, job: &Job, limit: Duration) -> Waited {
    let until = Instant::now() + limit;
    loop {
        if job.cancelled() {
            return Waited::Cancelled;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Waited::Silent;
        }
        match replies.recv_timeout(left.min(TICK)) {
            Ok(reply) => return Waited::Answered(reply),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Waited::Silent,
        }
    }
}

fn refused(message: &str) -> OpOutcome {
    OpOutcome { items: Vec::new(), error: Some(PlatformError::new(-1, message.to_owned())) }
}

pub fn queued(dest: &FsPath, data: &DroppedFile, job: &Job) -> Option<OpOutcome> {
    let promise = take(data)?;
    Some(settle(dest, promise, job, WAIT))
}

fn settle(dest: &FsPath, promise: Promise, job: &Job, limit: Duration) -> OpOutcome {
    job.update(|p| {
        p.stage = Stage::Preparing;
        p.total_items = 1;
    });
    let Promise { replies, staging, fallback, .. } = promise;
    match wait(&replies, job, limit) {
        Waited::Cancelled => crate::dropped::failed(&std::io::Error::from_raw_os_error(libc::ECANCELED)),
        Waited::Answered(Reply::Saved) => crate::dropped::copy_staged(&staging.file, dest, job),
        Waited::Answered(Reply::Declined) => match fallback {
            Some(content) => crate::dropped::queued(dest, &content, job),
            None => refused(S::UiDroppedSaveFailed.t()),
        },
        Waited::Answered(Reply::Failed) => refused(S::UiDroppedSaveFailed.t()),
        Waited::Silent => refused(S::UiDroppedSaveSilent.t()),
    }
}

#[cfg(test)]
#[path = "../test/xds.rs"]
mod tests;
