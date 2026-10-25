use crate::fs::{host, stat_path};
use flit_core::listing::{ChangeKind, DirChange};
use flit_core::{FsPath, ProviderEvent, TabId};
use flit_platform::{PlatformError, Wake, Watcher};
use crossbeam_channel::Sender;
use std::collections::{HashMap, HashSet};
use std::ffi::{CString, OsStr};
use std::hash::Hash;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const COALESCE: Duration = Duration::from_millis(50);
const MASK: u32 = libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO | libc::IN_MODIFY | libc::IN_ATTRIB | libc::IN_CLOSE_WRITE | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_ONLYDIR;
const RESCAN: u32 = libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED;
const HEADER: usize = 16;

pub enum Notice {
    Changes(Vec<DirChange>),
    Rescan,
}

struct Watches<K> {
    dirs: HashMap<i32, (PathBuf, Vec<K>)>,
    keys: HashMap<K, i32>,
}

pub struct Notify<K> {
    fd: Arc<OwnedFd>,
    watches: Arc<Mutex<Watches<K>>>,
}

#[derive(Default)]
struct Pending {
    names: Vec<Vec<u8>>,
    seen: HashSet<Vec<u8>>,
    rescan: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<K: Clone + Eq + Hash + Send + 'static> Notify<K> {
    pub fn new(emit: impl Fn(K, Notice) + Send + 'static) -> io::Result<Self> {
        Self::with_window(COALESCE, emit)
    }

    pub fn with_window(window: Duration, emit: impl Fn(K, Notice) + Send + 'static) -> io::Result<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = Arc::new(unsafe { OwnedFd::from_raw_fd(fd) });
        let watches = Arc::new(Mutex::new(Watches { dirs: HashMap::new(), keys: HashMap::new() }));
        let (rfd, rw) = (fd.clone(), watches.clone());
        std::thread::Builder::new().name("flit-inotify".into()).spawn(move || read_loop(&rfd, &rw, window, &emit))?;
        Ok(Self { fd, watches })
    }

    pub fn watch(&self, key: K, dir: &Path) -> io::Result<()> {
        self.unwatch(&key);
        let c = CString::new(dir.as_os_str().as_bytes()).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
        let mut w = lock(&self.watches);
        let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), c.as_ptr(), MASK) };
        if wd < 0 {
            return Err(io::Error::last_os_error());
        }
        w.dirs.entry(wd).or_insert_with(|| (dir.to_path_buf(), Vec::new())).1.push(key.clone());
        w.keys.insert(key, wd);
        Ok(())
    }

    pub fn unwatch(&self, key: &K) -> bool {
        let mut w = lock(&self.watches);
        let Some(wd) = w.keys.remove(key) else { return false };
        if let Some((_, keys)) = w.dirs.get_mut(&wd) {
            keys.retain(|k| k != key);
            if keys.is_empty() {
                w.dirs.remove(&wd);
                unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), wd) };
            }
        }
        true
    }
}

fn parse(buf: &[u8], pending: &mut HashMap<i32, Pending>, overflow: &mut bool) {
    let word = |at: usize| [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]];
    let mut off = 0;
    while off + HEADER <= buf.len() {
        let (wd, mask, len) = (i32::from_ne_bytes(word(off)), u32::from_ne_bytes(word(off + 4)), u32::from_ne_bytes(word(off + 12)) as usize);
        let raw = &buf[off + HEADER..(off + HEADER + len).min(buf.len())];
        let name = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
        off += HEADER + len;
        if mask & libc::IN_Q_OVERFLOW != 0 {
            *overflow = true;
            continue;
        }
        let p = pending.entry(wd).or_default();
        if mask & RESCAN != 0 {
            p.rescan = true;
        } else if !name.is_empty() && p.seen.insert(name.to_vec()) {
            p.names.push(name.to_vec());
        }
    }
}

fn flush<K: Clone>(watches: &Mutex<Watches<K>>, pending: HashMap<i32, Pending>, overflow: bool, emit: &dyn Fn(K, Notice)) {
    let targets: Vec<(PathBuf, Vec<K>, Pending)> = {
        let w = lock(watches);
        if overflow {
            w.dirs.values().flat_map(|(_, keys)| keys.iter().cloned()).for_each(|k| emit(k, Notice::Rescan));
            return;
        }
        pending.into_iter().filter_map(|(wd, p)| w.dirs.get(&wd).map(|(d, k)| (d.clone(), k.clone(), p))).collect()
    };
    for (dir, keys, p) in targets {
        if p.rescan {
            keys.into_iter().for_each(|k| emit(k, Notice::Rescan));
            continue;
        }
        let changes: Vec<DirChange> = p
            .names
            .into_iter()
            .map(|name| match stat_path(&dir.join(OsStr::from_bytes(&name))) {
                Ok(meta) => DirChange { kind: ChangeKind::Added, name, meta: Some(meta) },
                Err(_) => DirChange { kind: ChangeKind::Removed, name, meta: None },
            })
            .collect();
        keys.into_iter().for_each(|k| emit(k, Notice::Changes(changes.clone())));
    }
}

fn read_loop<K: Clone>(fd: &OwnedFd, watches: &Mutex<Watches<K>>, window: Duration, emit: &dyn Fn(K, Notice)) {
    let mut buf = vec![0u8; 64 << 10];
    let mut pending = HashMap::new();
    let mut overflow = false;
    let mut deadline: Option<Instant> = None;
    loop {
        let timeout = deadline.map_or(-1, |d| d.saturating_duration_since(Instant::now()).as_micros().div_ceil(1000) as i32);
        let mut pfd = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&mut pfd, 1, timeout) };
        if ready > 0 {
            let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            parse(&buf[..n as usize], &mut pending, &mut overflow);
            deadline.get_or_insert_with(|| Instant::now() + window);
        } else if ready < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return;
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            flush(watches, std::mem::take(&mut pending), std::mem::take(&mut overflow), emit);
            deadline = None;
        }
    }
}

pub struct Inotify {
    notify: Notify<(TabId, u64)>,
    current: HashMap<TabId, u64>,
}

impl Watcher for Inotify {
    fn new(events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError> {
        let notify = Notify::new(move |(tab, gen), notice| {
            let _ = events.send(match notice {
                Notice::Changes(changes) => ProviderEvent::Changes { tab, gen, changes },
                Notice::Rescan => ProviderEvent::Rescan { tab, gen },
            });
            wake();
        })
        .map_err(|e| PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("inotify: {e}")))?;
        Ok(Self { notify, current: HashMap::new() })
    }

    fn watch(&mut self, tab: TabId, gen: u64, dir: Option<&FsPath>) {
        if let Some(old) = self.current.remove(&tab) {
            self.notify.unwatch(&(tab, old));
        }
        let Some(path) = dir.and_then(host) else { return };
        match self.notify.watch((tab, gen), path) {
            Ok(()) => {
                self.current.insert(tab, gen);
            }
            Err(e) => eprintln!("flit: cannot watch {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
#[path = "../test/inotify.rs"]
mod tests;
