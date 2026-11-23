use crossbeam_channel::{unbounded, RecvTimeoutError, Sender};
use flit_core::{ProviderEvent, TabId};
use flit_platform::Wake;
use std::collections::HashMap;
use std::ffi::CString;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const PERIOD: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

enum Cmd {
    Watch(TabId, u64, PathBuf),
    Forget(TabId),
}

pub struct Poll {
    tx: Option<Sender<Cmd>>,
    events: Sender<ProviderEvent>,
    wake: Wake,
    period: Duration,
}

fn stamp(dir: &Path) -> Option<Stamp> {
    let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(std::os::fd::AsRawFd::as_raw_fd(&fd), &mut st) } != 0 {
        return None;
    }
    Some(Stamp { dev: st.st_dev, ino: st.st_ino, mtime: (st.st_mtime, st.st_mtime_nsec), ctime: (st.st_ctime, st.st_ctime_nsec) })
}

impl Poll {
    pub fn new(events: Sender<ProviderEvent>, wake: Wake, period: Duration) -> Self {
        Self { tx: None, events, wake, period }
    }

    pub fn watch(&mut self, tab: TabId, gen: u64, dir: &Path) {
        if self.tx.is_none() {
            let (tx, rx) = unbounded();
            let (events, wake, period) = (self.events.clone(), self.wake.clone(), self.period);
            let spawned = std::thread::Builder::new().name("flit-poll".into()).spawn(move || {
                let mut dirs: HashMap<TabId, (u64, PathBuf, Stamp)> = HashMap::new();
                let mut next = Instant::now() + period;
                loop {
                    match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                        Ok(Cmd::Watch(tab, gen, dir)) => {
                            dirs.remove(&tab);
                            if let Some(now) = (!crate::places::inactive_share(&dir)).then(|| stamp(&dir)).flatten() {
                                dirs.insert(tab, (gen, dir, now));
                            }
                        }
                        Ok(Cmd::Forget(tab)) => {
                            dirs.remove(&tab);
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            dirs.retain(|&tab, (gen, dir, seen)| {
                                if crate::places::inactive_share(dir) {
                                    return false;
                                }
                                let now = stamp(dir);
                                if now != Some(*seen) && events.send(ProviderEvent::Rescan { tab, gen: *gen }).is_ok() {
                                    wake();
                                }
                                if let Some(stamp) = now {
                                    *seen = stamp;
                                }
                                now.is_some()
                            });
                            next = Instant::now() + period;
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            });
            if spawned.is_err() {
                return;
            }
            self.tx = Some(tx);
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::Watch(tab, gen, dir.to_path_buf()));
        }
    }

    pub fn forget(&mut self, tab: TabId) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::Forget(tab));
        }
    }

    #[cfg(test)]
    pub fn running(&self) -> bool {
        self.tx.is_some()
    }
}

#[cfg(test)]
#[path = "../test/poll.rs"]
mod tests;
