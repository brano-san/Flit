use crate::places;
use crossbeam_channel::Sender;
use flit_core::listing::DirStats;
use flit_core::path::PathKind;
use flit_core::ui::WatchLimit;
use flit_core::{FsError, FsPath, ListingBuilder, ProviderEvent, TabId};
use flit_platform::{Fs, PlatformError, Wake, Watcher};
use flit_platform_linux::{host, LinuxFs, Notice, Notify};
use std::collections::HashMap;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const COALESCE: Duration = Duration::from_millis(16);

pub const WATCH_LIMIT_FILE: &str = "/proc/sys/fs/inotify/max_user_watches";
pub const WATCH_LIMIT_TARGET: u64 = 524_288;
pub const WATCH_LIMIT_CONF: &str = "/etc/sysctl.d/60-flit-inotify.conf";
const PKEXEC_DISMISSED: i32 = 126;
const RAISE_SCRIPT: &str = "printf 'fs.inotify.max_user_watches = %s\n' \"$1\" > \"$2\" && sysctl -q -w fs.inotify.max_user_watches=\"$1\"";

pub fn raised_limit(current: Option<u64>) -> u64 {
    current.map_or(WATCH_LIMIT_TARGET, |c| if c < WATCH_LIMIT_TARGET { WATCH_LIMIT_TARGET } else { c.saturating_mul(2) })
}

pub fn raise_args(limit: u64) -> Vec<String> {
    ["/bin/sh", "-c", RAISE_SCRIPT, "sh"].into_iter().map(str::to_owned).chain([limit.to_string(), WATCH_LIMIT_CONF.to_owned()]).collect()
}

pub fn raise_outcome(limit: u64, code: Option<i32>, stderr: &str) -> WatchLimit {
    match code {
        Some(0) => WatchLimit::Raised(limit),
        Some(PKEXEC_DISMISSED) => WatchLimit::Cancelled,
        _ if !stderr.trim().is_empty() => WatchLimit::Failed(stderr.trim().to_owned()),
        Some(c) => WatchLimit::Failed(format!("pkexec exit code {c}")),
        None => WatchLimit::Failed("pkexec was stopped by a signal".to_owned()),
    }
}

pub fn raise_watch_limit() -> WatchLimit {
    let pkexec = match crate::elevated::pkexec() {
        Ok(program) => program,
        Err(e) => return WatchLimit::Failed(e.message),
    };
    let current = std::fs::read_to_string(WATCH_LIMIT_FILE).ok().and_then(|t| t.trim().parse().ok());
    let limit = raised_limit(current);
    match std::process::Command::new(pkexec).args(raise_args(limit)).stdin(std::process::Stdio::null()).output() {
        Ok(out) => raise_outcome(limit, out.status.code(), &String::from_utf8_lossy(&out.stderr)),
        Err(e) => WatchLimit::Failed(format!("pkexec: {}", flit_core::fmt::io_reason(&e))),
    }
}

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
            PathKind::Shell if dir.is_network() => crate::network::read_dir(dir, out),
            _ => flit_archive::read_dir(dir, out, <crate::Linux as flit_platform::Platform>::utc_offset_minutes()).unwrap_or_else(|| LinuxFs.read_dir(dir, out)),
        }
    }

    fn seek_penalty(&self, path: &FsPath) -> Option<bool> {
        LinuxFs.seek_penalty(path)
    }

    fn remote(&self, path: &FsPath) -> bool {
        path.is_network() || LinuxFs.remote(path)
    }

    fn dir_sizes(&self, dir: &FsPath, items: &[(u32, Vec<u8>)], keep_going: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, DirStats) + Sync)) {
        LinuxFs.dir_sizes(dir, items, keep_going, report)
    }

    fn read_deep(&self, root: &FsPath, out: &mut ListingBuilder<'_>, keep_going: &dyn Fn() -> bool) {
        match flit_archive::locate(root) {
            Some(_) => flit_platform::read_deep(self, root, out, keep_going),
            None => LinuxFs.read_deep(root, out, keep_going),
        }
    }

    fn volume_size(&self, path: &FsPath) -> u64 {
        LinuxFs.volume_size(path)
    }

    fn desktop_ini(&self, dir: &FsPath) -> Option<flit_core::desktop_ini::DesktopIni> {
        let kind = crate::xdg::folder_kind(host(dir)?)?;
        Some(flit_core::desktop_ini::DesktopIni { kind, ..Default::default() })
    }

    fn cluster_size(&self, path: &FsPath) -> u64 {
        LinuxFs.cluster_size(path)
    }

    fn measure(&self, items: &[(FsPath, Option<u64>)], keep_going: &dyn Fn() -> bool, report: &mut dyn FnMut(&flit_core::provider::Totals)) -> flit_core::provider::Totals {
        if items.iter().all(|(p, _)| p.kind() == PathKind::Unix && flit_archive::locate(p).is_none()) {
            LinuxFs.measure(items, keep_going, report)
        } else {
            flit_platform::measure(self, items, keep_going, report)
        }
    }

    fn file_system(&self, path: &FsPath) -> Option<String> {
        LinuxFs.file_system(path)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Spot {
    Folder,
    Trash(usize),
}

type TrashTabs = Arc<Mutex<HashMap<TabId, u64>>>;

type Folders = Arc<Mutex<HashMap<TabId, (u64, std::path::PathBuf)>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn folder_icon_changed(folders: &Folders, tab: TabId, gen: u64, changes: &[flit_core::listing::DirChange]) {
    if !changes.iter().any(|c| c.name == b".directory") {
        return;
    }
    let Some(dir) = lock(folders).get(&tab).filter(|(g, _)| *g == gen).map(|(_, p)| p.clone()) else { return };
    crate::folder_icons::forget(&dir);
    crate::window::icons_changed();
}

fn rescan_trash(tabs: &TrashTabs, events: &Sender<ProviderEvent>, wake: &Wake) {
    let shown: Vec<(TabId, u64)> = tabs.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(&t, &g)| (t, g)).collect();
    let sent = shown.into_iter().filter(|&(tab, gen)| events.send(ProviderEvent::Rescan { tab, gen }).is_ok()).count();
    if sent > 0 {
        wake();
    }
}

pub struct LinuxGuiWatcher {
    notify: Notify<(TabId, u64, Spot)>,
    folders: Folders,
    poll: crate::poll::Poll,
    remote: fn(&FsPath) -> bool,
    current: HashMap<TabId, (u64, Vec<Spot>)>,
    trash: Option<flit_platform_linux::Trash>,
    trash_tabs: TrashTabs,
    mounts_watched: bool,
    events: Sender<ProviderEvent>,
    wake: Wake,
}

impl LinuxGuiWatcher {
    fn blind(&self, tab: TabId, gen: u64, e: &io::Error) {
        tracing::warn!("live updates are off for tab {tab}: {e}");
        let limit = e.raw_os_error() == Some(libc::ENOSPC);
        if self.events.send(ProviderEvent::Unwatched { tab, gen, limit }).is_ok() {
            (self.wake)();
        }
        blind_tabs().push(Blind { tab, gen, events: self.events.clone(), wake: self.wake.clone() });
    }

    fn watch_mounts(&mut self) {
        if self.mounts_watched {
            return;
        }
        self.mounts_watched = true;
        let (tabs, events, wake) = (self.trash_tabs.clone(), self.events.clone(), self.wake.clone());
        if let Err(e) = flit_platform_linux::mounts::watch(Path::new(flit_platform_linux::mounts::MOUNTINFO), move || rescan_trash(&tabs, &events, &wake)) {
            tracing::warn!("the trash view will not notice new drives: {e}");
        }
    }

    fn watch_trash(&mut self, tab: TabId, gen: u64) {
        if self.trash.is_none() {
            return;
        }
        self.trash_tabs.lock().unwrap_or_else(|e| e.into_inner()).insert(tab, gen);
        self.watch_mounts();
        let (tops, volumes) = places::trash_tops();
        let folders = self.trash.as_ref().map(|t| places::trash_folders(t, &tops, &volumes)).unwrap_or_default();
        let mut spots = Vec::new();
        for (k, folder) in folders.iter().enumerate() {
            if self.notify.watch((tab, gen, Spot::Trash(k)), folder).is_ok() {
                spots.push(Spot::Trash(k));
            }
        }
        self.current.insert(tab, (gen, spots));
    }
}

impl Watcher for LinuxGuiWatcher {
    fn new(events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError> {
        let (sink, ping, folders) = (events.clone(), wake.clone(), Folders::default());
        let shown = folders.clone();
        let notify = Notify::with_window(COALESCE, move |(tab, gen, spot), notice| {
            let _ = sink.send(match (spot, notice) {
                (Spot::Folder, Notice::Changes(changes)) => {
                    folder_icon_changed(&shown, tab, gen, &changes);
                    ProviderEvent::Changes { tab, gen, changes }
                }
                _ => ProviderEvent::Rescan { tab, gen },
            });
            ping();
        })
        .map_err(|e| PlatformError::from_io(&e, format!("inotify: {e}")))?;
        let poll = crate::poll::Poll::new(events.clone(), wake.clone(), crate::poll::PERIOD);
        Ok(Self { notify, folders, poll, remote: |dir| LinuxFs.remote(dir), current: HashMap::new(), trash: places::trash().ok(), trash_tabs: TrashTabs::default(), mounts_watched: false, events, wake })
    }

    fn watch(&mut self, tab: TabId, gen: u64, dir: Option<&FsPath>) {
        if let Some((old, spots)) = self.current.remove(&tab) {
            for spot in spots {
                self.notify.unwatch(&(tab, old, spot));
            }
        }
        lock(&self.folders).remove(&tab);
        blind_tabs().retain(|b| b.tab != tab);
        self.poll.forget(tab);
        self.trash_tabs.lock().unwrap_or_else(|e| e.into_inner()).remove(&tab);
        if dir.is_some_and(|d| d.kind() == PathKind::RecycleBin) {
            return self.watch_trash(tab, gen);
        }
        let Some(path) = dir.and_then(host) else { return };
        if dir.is_some_and(self.remote) {
            self.poll.watch(tab, gen, path);
        }
        match self.notify.watch((tab, gen, Spot::Folder), path) {
            Ok(()) => {
                self.current.insert(tab, (gen, vec![Spot::Folder]));
                lock(&self.folders).insert(tab, (gen, path.to_path_buf()));
            }
            Err(e) => self.blind(tab, gen, &e),
        }
    }
}

#[cfg(test)]
#[path = "../test/fs.rs"]
mod tests;
