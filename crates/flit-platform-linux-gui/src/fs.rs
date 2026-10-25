use crate::places;
use crossbeam_channel::Sender;
use flit_core::path::PathKind;
use flit_core::{FsError, FsPath, ListingBuilder, ProviderEvent, TabId};
use flit_platform::{Fs, PlatformError, Wake, Watcher};
use flit_platform_linux::{host, LinuxFs, Notice, Notify};
use std::collections::HashMap;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

pub const COALESCE: Duration = Duration::from_millis(16);

pub const WATCH_LIMIT: &str = "Live updates are off: the inotify watch limit (fs.inotify.max_user_watches) is reached, so this folder refreshes after Flit's own operations";

pub struct LinuxGuiFs;

fn device(mut p: &Path) -> Option<u64> {
    loop {
        match std::fs::metadata(p) {
            Ok(m) => return Some(m.dev()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => p = p.parent()?,
            Err(_) => return None,
        }
    }
}

impl Fs for LinuxGuiFs {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        match dir.kind() {
            PathKind::Computer => places::list_computer(out),
            PathKind::RecycleBin => places::list_trash(out),
            _ => LinuxFs.read_dir(dir, out),
        }
    }

    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }

    fn dir_sizes(&self, dir: &FsPath, items: &[(u32, Vec<u8>)], keep_going: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, u64) + Sync)) {
        LinuxFs.dir_sizes(dir, items, keep_going, report)
    }

    fn volume_key(&self, path: &FsPath) -> Vec<u8> {
        match host(path).and_then(device) {
            Some(dev) => format!("dev:{dev}").into_bytes(),
            None => path.root_bytes().to_vec(),
        }
    }
}

struct Blind {
    tab: TabId,
    gen: u64,
    events: Sender<ProviderEvent>,
    wake: Wake,
}

static BLIND: Mutex<Vec<Blind>> = Mutex::new(Vec::new());

fn blind_tabs() -> std::sync::MutexGuard<'static, Vec<Blind>> {
    BLIND.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn rescan_blind() {
    for b in std::mem::take(&mut *blind_tabs()) {
        if b.events.send(ProviderEvent::Rescan { tab: b.tab, gen: b.gen }).is_ok() {
            (b.wake)();
        }
    }
}

pub struct LinuxGuiWatcher {
    notify: Notify<(TabId, u64)>,
    current: HashMap<TabId, u64>,
    events: Sender<ProviderEvent>,
    wake: Wake,
}

impl LinuxGuiWatcher {
    fn blind(&self, tab: TabId, gen: u64, e: &io::Error) {
        let text = if e.raw_os_error() == Some(libc::ENOSPC) { WATCH_LIMIT.to_owned() } else { format!("Live updates are off for this folder: {e}") };
        if self.events.send(ProviderEvent::Status { tab, gen, text }).is_ok() {
            (self.wake)();
        }
        blind_tabs().push(Blind { tab, gen, events: self.events.clone(), wake: self.wake.clone() });
    }
}

impl Watcher for LinuxGuiWatcher {
    fn new(events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError> {
        let (sink, ping) = (events.clone(), wake.clone());
        let notify = Notify::with_window(COALESCE, move |(tab, gen), notice| {
            let _ = sink.send(match notice {
                Notice::Changes(changes) => ProviderEvent::Changes { tab, gen, changes },
                Notice::Rescan => ProviderEvent::Rescan { tab, gen },
            });
            ping();
        })
        .map_err(|e| PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("inotify: {e}")))?;
        Ok(Self { notify, current: HashMap::new(), events, wake })
    }

    fn watch(&mut self, tab: TabId, gen: u64, dir: Option<&FsPath>) {
        if let Some(old) = self.current.remove(&tab) {
            self.notify.unwatch(&(tab, old));
        }
        blind_tabs().retain(|b| b.tab != tab);
        let Some(path) = dir.and_then(host) else { return };
        match self.notify.watch((tab, gen), path) {
            Ok(()) => {
                self.current.insert(tab, gen);
            }
            Err(e) => self.blind(tab, gen, &e),
        }
    }
}

#[cfg(test)]
#[path = "../test/fs.rs"]
mod tests;
