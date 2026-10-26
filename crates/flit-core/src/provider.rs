use crate::archive::ArchiveOp;
use crate::digest::HashKind;
use crate::desktop_ini::FolderKind;
use crate::listing::{DirChange, DirSnapshot, Meta};
use crate::path::FsPath;
use crate::sort::{CaseMode, ViewSpec};
use arc_swap::ArcSwapOption;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

pub type TabId = u64;

const KIND_ASKED: u64 = 0xFF;

#[derive(Default)]
pub struct SnapshotSlot {
    snap: ArcSwapOption<DirSnapshot>,
    kind: AtomicU64,
}

impl SnapshotSlot {
    pub fn publish(&self, snap: Arc<DirSnapshot>) {
        self.snap.rcu(|cur| match cur {
            Some(newer) if newer.gen > snap.gen => Some(newer.clone()),
            _ => Some(snap.clone()),
        });
    }
    pub fn take(&self) -> Option<Arc<DirSnapshot>> {
        self.snap.swap(None)
    }
    pub fn ask_kind(&self, gen: u64) {
        self.kind.store(gen << 8 | KIND_ASKED, Ordering::Release);
    }
    pub fn wants_kind(&self, gen: u64) -> bool {
        self.kind.load(Ordering::Acquire) == gen << 8 | KIND_ASKED
    }
    pub fn set_kind(&self, gen: u64, kind: FolderKind) {
        let _ = self.kind.compare_exchange(gen << 8 | KIND_ASKED, gen << 8 | kind as u64, Ordering::AcqRel, Ordering::Acquire);
    }
    pub fn kind(&self, gen: u64) -> FolderKind {
        let v = self.kind.load(Ordering::Acquire);
        if v >> 8 == gen && v & KIND_ASKED != KIND_ASKED { FolderKind::from_u8(v as u8) } else { FolderKind::Unknown }
    }
}

impl fmt::Debug for SnapshotSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SnapshotSlot")
    }
}

#[derive(Clone, Default, Debug)]
pub struct GenToken(Arc<AtomicU64>);

impl GenToken {
    pub fn set(&self, gen: u64) {
        self.0.store(gen, Ordering::Release);
    }
    pub fn is_current(&self, gen: u64) -> bool {
        self.0.load(Ordering::Acquire) == gen
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Template {
    Empty,
    Copy(FsPath),
    Data(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileOp {
    Trash(Vec<FsPath>),
    Copy { items: Vec<FsPath>, dest: FsPath },
    Move { items: Vec<FsPath>, dest: FsPath },
    Link { items: Vec<FsPath>, dest: FsPath },
    Rename { item: FsPath, name: Vec<u8> },
    NewFolder { dir: FsPath, name: Vec<u8> },
    NewFile { dir: FsPath, name: Vec<u8>, template: Template },
    NewStream { item: FsPath, name: Vec<u8> },
    MoveTo(Vec<(FsPath, FsPath)>),
    Restore(Vec<(FsPath, FsPath)>),
    Paste { dest: FsPath },
    Delete(Vec<FsPath>),
    Purge(Vec<(FsPath, FsPath)>),
    EmptyBin,
    Flatten(Vec<FsPath>),
    Archive(ArchiveOp),
    FolderIcon { folder: FsPath, icon: Option<FsPath>, index: i32 },
    InstallFonts(Vec<FsPath>),
    LockScreen(FsPath),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    Terminal { dirs: Vec<FsPath>, admin: bool },
    With { program: FsPath, args: Vec<FsPath> },
    Run { program: String, args: String, dir: Option<FsPath> },
    Eject(FsPath),
    Mount(FsPath),
}

impl Launch {
    pub fn map_paths(&self, f: impl Fn(&FsPath) -> Option<FsPath>) -> Option<Launch> {
        let all = |v: &[FsPath]| v.iter().map(&f).collect::<Option<Vec<_>>>();
        Some(match self {
            Launch::Terminal { dirs, admin } => Launch::Terminal { dirs: all(dirs)?, admin: *admin },
            Launch::With { program, args } => Launch::With { program: f(program)?, args: all(args)? },
            Launch::Run { program, args, dir } => Launch::Run { program: program.clone(), args: args.clone(), dir: dir.as_ref().map(&f).unwrap_or(None) },
            Launch::Eject(drive) => Launch::Eject(f(drive)?),
            Launch::Mount(device) => Launch::Mount(device.clone()),
        })
    }
}

impl FileOp {
    pub fn all_paths(&self, f: impl Fn(&FsPath) -> bool) -> bool {
        self.map_paths(|p| f(p).then(|| p.clone())).is_some()
    }

    pub fn map_paths(&self, f: impl Fn(&FsPath) -> Option<FsPath>) -> Option<FileOp> {
        let all = |v: &[FsPath]| v.iter().map(&f).collect::<Option<Vec<_>>>();
        let pairs = |v: &[(FsPath, FsPath)]| v.iter().map(|(a, b)| Some((f(a)?, f(b)?))).collect::<Option<Vec<_>>>();
        Some(match self {
            FileOp::Trash(v) => FileOp::Trash(all(v)?),
            FileOp::Copy { items, dest } => FileOp::Copy { items: all(items)?, dest: f(dest)? },
            FileOp::Move { items, dest } => FileOp::Move { items: all(items)?, dest: f(dest)? },
            FileOp::Link { items, dest } => FileOp::Link { items: all(items)?, dest: f(dest)? },
            FileOp::Rename { item, name } => FileOp::Rename { item: f(item)?, name: name.clone() },
            FileOp::NewFolder { dir, name } => FileOp::NewFolder { dir: f(dir)?, name: name.clone() },
            FileOp::NewFile { dir, name, template } => FileOp::NewFile { dir: f(dir)?, name: name.clone(), template: template.clone() },
            FileOp::NewStream { item, name } => FileOp::NewStream { item: f(item)?, name: name.clone() },
            FileOp::MoveTo(v) => FileOp::MoveTo(pairs(v)?),
            FileOp::Restore(v) => FileOp::Restore(pairs(v)?),
            FileOp::Paste { dest } => FileOp::Paste { dest: f(dest)? },
            FileOp::Delete(v) => FileOp::Delete(all(v)?),
            FileOp::Purge(v) => FileOp::Purge(pairs(v)?),
            FileOp::EmptyBin => FileOp::EmptyBin,
            FileOp::Flatten(v) => FileOp::Flatten(all(v)?),
            FileOp::Archive(a) => FileOp::Archive(a.map_paths(&f)?),
            FileOp::FolderIcon { folder, icon, index } => FileOp::FolderIcon { folder: f(folder)?, icon: match icon { Some(i) => Some(f(i)?), None => None }, index: *index },
            FileOp::InstallFonts(v) => FileOp::InstallFonts(all(v)?),
            FileOp::LockScreen(p) => FileOp::LockScreen(f(p)?),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Resolve {
    KeepBoth,
    Replace,
    Skip,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Conflict {
    pub source: FsPath,
    pub target: FsPath,
    pub from: Meta,
    pub to: Meta,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Stage {
    #[default]
    Preparing,
    Queued,
    Running,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpProgress {
    pub stage: Stage,
    pub bytes: u64,
    pub total_bytes: u64,
    pub items: u64,
    pub total_items: u64,
}

#[derive(Default, Debug)]
struct Answer {
    policy: Option<Resolve>,
    given: Option<(Resolve, bool)>,
}

#[derive(Default, Debug)]
struct Control {
    cancelled: AtomicBool,
    answer: Mutex<Answer>,
    changed: Condvar,
}

#[derive(Clone, Default, Debug)]
pub struct OpCtl(Arc<Control>);

impl OpCtl {
    pub fn new(policy: Option<Resolve>) -> Self {
        let ctl = Self::default();
        ctl.lock().policy = policy;
        ctl
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Answer> {
        self.0.answer.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        let _held = self.lock();
        self.0.changed.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn policy(&self) -> Option<Resolve> {
        self.lock().policy
    }

    pub fn answer(&self, choice: Resolve, all: bool) {
        self.lock().given = Some((choice, all));
        self.0.changed.notify_all();
    }

    pub fn resolve(&self, ask: impl FnOnce()) -> Option<Resolve> {
        let mut a = self.lock();
        if let Some(p) = a.policy {
            return Some(p);
        }
        a.given = None;
        drop(a);
        if self.is_cancelled() {
            return None;
        }
        ask();
        let mut a = self.lock();
        while a.given.is_none() && !self.is_cancelled() {
            a = self.0.changed.wait(a).unwrap_or_else(|e| e.into_inner());
        }
        let (choice, all) = a.given.take().filter(|_| !self.is_cancelled())?;
        if all {
            a.policy = Some(choice);
        }
        Some(choice)
    }
}

pub type OpId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpReport {
    pub op: FileOp,
    pub items: Vec<(FsPath, FsPath)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u64,
    pub folders: u64,
    pub size: u64,
    pub on_disk: u64,
    pub unpacked: u64,
    pub accessed: i64,
}

#[derive(Debug)]
pub enum ProviderRequest {
    List { tab: TabId, gen: u64, path: FsPath, view: ViewSpec, case: CaseMode, out: Arc<SnapshotSlot>, cancel: GenToken },
    Watch { tab: TabId, gen: u64, path: Option<FsPath> },
    DirSizes { tab: TabId, gen: u64, dir: FsPath, items: Vec<(u32, Vec<u8>)>, cancel: GenToken },
    Op { tab: TabId, id: OpId, op: FileOp, ctl: OpCtl },
    Open { tab: TabId, path: FsPath },
    Launch { tab: TabId, launch: Launch },
    Hash { tab: TabId, id: OpId, file: FsPath, kinds: Vec<HashKind>, ctl: OpCtl },
    Measure { tab: TabId, id: OpId, items: Vec<(FsPath, Option<u64>)>, ctl: OpCtl },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    Changes { tab: TabId, gen: u64, changes: Vec<DirChange> },
    Rescan { tab: TabId, gen: u64 },
    DirSize { tab: TabId, gen: u64, idx: u32, size: u64 },
    OpDone { tab: TabId, id: OpId, error: Option<String>, report: Option<OpReport> },
    OpProgress { tab: TabId, id: OpId, progress: OpProgress },
    OpConflict { tab: TabId, id: OpId, conflict: Conflict },
    Navigate { tab: TabId, path: FsPath },
    Hashed { tab: TabId, id: OpId, result: Result<Vec<String>, String> },
    Measured { tab: TabId, id: OpId, totals: Totals, file_system: Option<String>, done: bool },
    Status { tab: TabId, gen: u64, text: String },
}

#[cfg(test)]
#[path = "../test/provider.rs"]
mod tests;
