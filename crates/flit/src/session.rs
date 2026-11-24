use crossbeam_channel::{unbounded, Sender};
use flit_core::Location;
use flit_core::ui::{PanelsSession, Startup, WindowSession};
use flit_platform::{Placement, ProfilePlatform, SystemPlatform};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::io::Write;
use std::thread::JoinHandle;

const VERSION: u32 = 1;
const SETTLED: std::time::Duration = std::time::Duration::from_secs(60);

pub fn restore_crash(startup: &Startup, unix: bool, settled: bool) -> bool {
    unix && settled && matches!(startup, Startup::Continue)
}

pub fn startup_windows(startup: &Startup, previous: Option<&Session>, restore_allowed: bool, restarted: bool) -> VecDeque<(WindowSession, Option<Placement>)> {
    if !restore_allowed {
        return VecDeque::new();
    }
    match startup {
        _ if restarted => previous.into_iter().flat_map(Session::windows).collect(),
        Startup::Continue => previous.into_iter().flat_map(Session::windows).collect(),
        Startup::NewTab => VecDeque::from([(WindowSession { tabs: vec![Location::home_page()], ..Default::default() }, None)]),
        Startup::Folders(tabs) => VecDeque::from([(WindowSession { tabs: tabs.clone(), ..Default::default() }, None)]),
    }
}

/// Map the requested window last so it gets focus even without a Wayland activation token.
pub fn open_requested_window_last(windows: &mut std::collections::VecDeque<(WindowSession, Option<Placement>)>, routes: &[flit_core::open::Route]) {
    let Some(mut target) = windows.len().checked_sub(1) else { return };
    for route in routes.iter().take(flit_core::ui::MAX_ROUTES) {
        if !windows[target].0.tabs.contains(&route.folder) {
            if let Some(found) = windows.iter().position(|(w, _)| w.tabs.contains(&route.folder)) {
                target = found;
            }
        }
    }
    if let Some(requested) = windows.remove(target) {
        windows.push_back(requested);
    }
}

type Bounds = (i32, i32, i32, i32, bool);

#[derive(Serialize, Deserialize)]
enum SplitDir {
    Vertical,
    Horizontal,
}

#[derive(Serialize, Deserialize)]
struct SplitSession {
    other: Location,
    dir: SplitDir,
    ratio: f32,
    second: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Session {
    version: u32,
    windows: Vec<(WindowSession, Option<Bounds>)>,
    pub typed: Vec<String>,
}

impl Session {
    pub fn new(windows: Vec<(WindowSession, Option<Placement>)>, typed: Vec<String>) -> Self {
        let windows = windows
            .into_iter()
            .map(|(mut w, p)| {
                for location in w.tabs.iter_mut().chain(&mut w.trails) {
                    *location = crate::state::compatible_location(location);
                }
                (w, p.map(|p| (p.x, p.y, p.width, p.height, p.maximized)))
            })
            .collect();
        Self { version: VERSION, windows, typed }
    }

    pub fn windows(&self) -> impl Iterator<Item = (WindowSession, Option<Placement>)> + '_ {
        self.windows.iter().filter(|(w, _)| !w.tabs.is_empty()).map(|(w, b)| (w.clone(), b.map(|(x, y, width, height, maximized)| Placement { x, y, width, height, maximized })))
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (mut s, tail) = postcard::take_from_bytes::<Self>(bytes).ok().filter(|(s, _)| s.version == VERSION)?;
        let Ok((_, tail)) = postcard::take_from_bytes::<Vec<Vec<Option<SplitSession>>>>(tail) else { return Some(s) };
        let Ok((trails, tail)) = postcard::take_from_bytes::<Vec<Vec<Location>>>(tail) else { return Some(s) };
        if trails.len() == s.windows.len() {
            s.windows.iter_mut().zip(trails).for_each(|((w, _), t)| w.trails = t);
        }
        if let Ok((panels, _)) = postcard::take_from_bytes::<Vec<Option<PanelsSession>>>(tail) {
            if panels.len() == s.windows.len() {
                s.windows.iter_mut().zip(panels).for_each(|((w, _), p)| w.panels = p);
            }
        }
        Some(s)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        let splits: Vec<Vec<Option<SplitSession>>> = self.windows.iter().map(|_| Vec::new()).collect();
        let trails: Vec<&Vec<Location>> = self.windows.iter().map(|(w, _)| &w.trails).collect();
        let panels: Vec<&Option<PanelsSession>> = self.windows.iter().map(|(w, _)| &w.panels).collect();
        postcard::to_stdvec(&(self, splits, trails, panels)).ok()
    }
}

pub struct Files {
    pub session: PathBuf,
    marker: PathBuf,
}

impl Files {
    pub fn of<P: ProfilePlatform + SystemPlatform>() -> Option<Self> {
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

    pub fn settled_before_exit(&self) -> bool {
        let saved = std::fs::metadata(&self.session).and_then(|m| m.modified()).ok();
        saved.zip(self.marked_at()).is_some_and(|(saved, started)| saved.duration_since(started).is_ok_and(|ran| ran >= SETTLED))
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

    pub fn finish(&self, snapshot: Option<&Session>) {
        if snapshot.is_none_or(|s| write(&self.session, s)) {
            self.unmark();
        }
    }
}

const MIGRATIONS: [crate::schema::Step; 0] = [];

pub fn load(path: &Path) -> Option<Session> {
    crate::schema::load(path, VERSION, &MIGRATIONS, Session::decode)
}

pub fn write(path: &Path, s: &Session) -> bool {
    store(path, s, true)
}

fn store(path: &Path, s: &Session, durable: bool) -> bool {
    let Some(bytes) = s.encode() else { return false };
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        if durable {
            file.sync_all()?;
        }
        drop(file);
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        if let Some(dir) = path.parent().filter(|_| durable) {
            std::fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        tracing::warn!("cannot save the session: {e}");
        let _ = std::fs::remove_file(tmp);
        return false;
    }
    true
}

pub struct Writer {
    tx: Option<Sender<(Session, Option<Sender<()>>)>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn start(path: PathBuf) -> Self {
        let (tx, rx) = unbounded::<(Session, Option<Sender<()>>)>();
        let thread = std::thread::Builder::new()
            .name("flit-session".into())
            .spawn(move || {
                while let Ok((mut s, ack)) = rx.recv() {
                    let mut acks: Vec<_> = ack.into_iter().collect();
                    while let Ok((newer, ack)) = rx.try_recv() {
                        s = newer;
                        acks.extend(ack);
                    }
                    store(&path, &s, !acks.is_empty());
                    for ack in acks {
                        let _ = ack.send(());
                    }
                }
            })
            .ok();
        Self { tx: thread.as_ref().map(|_| tx), thread }
    }

    pub fn send(&self, s: Session) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((s, None));
        }
    }

    pub fn flush(&self, s: Session) {
        if let Some(tx) = &self.tx {
            let (ack, done) = crossbeam_channel::bounded(1);
            if tx.send((s, Some(ack))).is_ok() {
                let _ = done.recv();
            }
        }
    }

    pub fn finish(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
#[path = "../test/session.rs"]
mod tests;
