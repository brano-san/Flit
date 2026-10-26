use crossbeam_channel::{unbounded, Sender};
use flit_core::ui::{SplitSession, WindowSession};
use flit_platform::{Placement, Platform};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

const VERSION: u32 = 1;

type Bounds = (i32, i32, i32, i32, bool);

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Session {
    version: u32,
    windows: Vec<(WindowSession, Option<Bounds>)>,
    pub typed: Vec<String>,
}

impl Session {
    pub fn new(windows: Vec<(WindowSession, Option<Placement>)>, typed: Vec<String>) -> Self {
        let windows = windows.into_iter().map(|(w, p)| (w, p.map(|p| (p.x, p.y, p.width, p.height, p.maximized)))).collect();
        Self { version: VERSION, windows, typed }
    }

    pub fn windows(&self) -> impl Iterator<Item = (WindowSession, Option<Placement>)> + '_ {
        self.windows.iter().filter(|(w, _)| !w.tabs.is_empty()).map(|(w, b)| (w.clone(), b.map(|(x, y, width, height, maximized)| Placement { x, y, width, height, maximized })))
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (mut s, tail) = postcard::take_from_bytes::<Self>(bytes).ok().filter(|(s, _)| s.version == VERSION)?;
        if let Ok((splits, _)) = postcard::take_from_bytes::<Vec<Vec<Option<SplitSession>>>>(tail) {
            if splits.len() == s.windows.len() {
                s.windows.iter_mut().zip(splits).for_each(|((w, _), panes)| w.splits = panes);
            }
        }
        Some(s)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        let mut bytes = postcard::to_stdvec(self).ok()?;
        let splits: Vec<&Vec<Option<SplitSession>>> = self.windows.iter().map(|(w, _)| &w.splits).collect();
        bytes.extend(postcard::to_stdvec(&splits).ok()?);
        Some(bytes)
    }
}

pub struct Files {
    pub session: PathBuf,
    marker: PathBuf,
}

impl Files {
    pub fn of<P: Platform>() -> Option<Self> {
        let dir = P::state_dir()?;
        let suffix = if P::elevated() { "-admin" } else { "" };
        Some(Self { session: dir.join(format!("session{suffix}.bin")), marker: dir.join(format!("running{suffix}")) })
    }

    pub fn crashed(&self) -> bool {
        self.marker.exists()
    }

    pub fn marked_at(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(&self.marker).and_then(|m| m.modified()).ok()
    }

    pub fn mark(&self) {
        if let Some(dir) = self.marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.marker, std::process::id().to_string());
    }

    pub fn unmark(&self) {
        let _ = std::fs::remove_file(&self.marker);
    }
}

const MIGRATIONS: [crate::schema::Step; 0] = [];

pub fn load(path: &Path) -> Option<Session> {
    crate::schema::load(path, VERSION, &MIGRATIONS, Session::decode)
}

pub fn write(path: &Path, s: &Session) {
    let Some(bytes) = s.encode() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

pub struct Writer {
    tx: Option<Sender<Session>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn start(path: PathBuf) -> Self {
        let (tx, rx) = unbounded::<Session>();
        let thread = std::thread::Builder::new()
            .name("flit-session".into())
            .spawn(move || {
                while let Ok(mut s) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        s = newer;
                    }
                    write(&path, &s);
                }
            })
            .ok();
        Self { tx: thread.as_ref().map(|_| tx), thread }
    }

    pub fn send(&self, s: Session) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(s);
        }
    }

    pub fn finish(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
#[path = "../test/session.rs"]
mod tests;
