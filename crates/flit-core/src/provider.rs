use crate::archive::ArchiveOp;
use crate::digest::HashKind;
use crate::desktop_ini::FolderKind;
use crate::listing::{DirChange, DirSnapshot, DirStats, Meta};
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Access {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

impl Access {
    pub const DIR: u32 = 0o040000;
    pub const LINK: u32 = 0o120000;
    pub const FILE: u32 = 0o100000;
    const KIND: u32 = 0o170000;

    pub fn kind(&self) -> u32 {
        self.mode & Self::KIND
    }

    pub fn perms(&self) -> u32 {
        self.mode & 0o7777
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModeEdit {
    pub set: u32,
    pub clear: u32,
}

impl ModeEdit {
    pub fn between(before: u32, after: u32) -> Self {
        let changed = (before ^ after) & 0o7777;
        Self { set: after & changed, clear: before & changed }
    }

    pub fn apply(self, perms: u32) -> u32 {
        (perms & !self.clear | self.set) & 0o7777
    }

    pub fn is_empty(self) -> bool {
        (self.set | self.clear) & 0o7777 == 0
    }

    pub fn only(self, mask: u32) -> Self {
        Self { set: self.set & mask, clear: self.clear & mask }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Enclosed {
    pub files: ModeEdit,
    pub folders: ModeEdit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccessEdit {
    pub mode: ModeEdit,
    pub owner: Option<u32>,
    pub group: Option<u32>,
    pub enclosed: Option<Enclosed>,
}

impl AccessEdit {
    pub fn is_empty(&self) -> bool {
        self.mode.is_empty() && self.owner.is_none() && self.group.is_none() && self.enclosed.is_none_or(|e| e.files.is_empty() && e.folders.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccessChange {
    pub item: FsPath,
    pub from: Access,
    pub to: Access,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Accounts {
    pub users: Vec<(u32, String)>,
    pub groups: Vec<(u32, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LinkKind {
    Shortcut,
    Symbolic,
    Junction,
    Hard,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkChoices(pub [bool; 4]);

impl LinkChoices {
    pub fn allows(self, kind: LinkKind) -> bool {
        self.0[kind as usize]
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DroppedFile {
    pub name: Vec<u8>,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for DroppedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DroppedFile").field("name", &crate::name::display(&self.name)).field("len", &self.bytes.len()).finish()
    }
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
    PasteLink { dest: FsPath },
    SetAccess { items: Vec<FsPath>, edit: AccessEdit },
    Access(Vec<AccessChange>),
    SetAttributes { items: Vec<FsPath>, set: u32, clear: u32, unblock: bool },
    PasteDropped { dest: FsPath, moving: bool, #[serde(skip)] data: Option<Arc<DroppedFile>> },
    GrantAccess(FsPath),
    LinkAs { items: Vec<FsPath>, dest: FsPath, kind: LinkKind },
    PasteLinkAs { dest: FsPath, kind: LinkKind },
    Segments { source: FsPath, dest: FsPath, part_size: Option<u64> },
    Dedup { keep: FsPath, candidates: Vec<FsPath>, backups: Vec<DedupBackup> },
    RestoreDedup { keep: FsPath, backups: Vec<DedupBackup> },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DedupBackup {
    pub target: FsPath,
    pub trash: FsPath,
    pub linked: crate::tags::FileKey,
    pub saved: crate::tags::FileKey,
}

impl DedupBackup {
    fn map(&self, f: &impl Fn(&FsPath) -> Option<FsPath>) -> Option<Self> {
        Some(Self { target: f(&self.target)?, trash: f(&self.trash)?, ..self.clone() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    Terminal { dirs: Vec<FsPath>, admin: bool },
    RunInTerminal { program: FsPath },
    With { program: FsPath, args: Vec<FsPath> },
    Run { program: String, args: String, dir: Option<FsPath> },
    Eject(FsPath),
    Unmount(FsPath),
    Mount(FsPath),
    Unlock { device: FsPath, passphrase: crate::archive::Secret },
    MapDrive(Option<FsPath>),
    Disconnect(Option<FsPath>),
    ConnectServer(String),
    ConnectAndSaveServer(String),
    ConnectNetwork { uri: String, gen: u64 },
    Reveal { item: FsPath, new_tab: bool },
    ChooseApp { files: Vec<FsPath>, mime: String, apps: Vec<AppChoice> },
    SetDefault { program: FsPath, mime: String },
    Custom { command: String, files: Vec<FsPath>, mime: String, remember: bool },
    Execute(FsPath),
    Display(FsPath),
    Print(Vec<FsPath>),
    FileManager(FsPath),
    Command { runs: Vec<CommandRun>, dir: Option<FsPath> },
    Wallpaper(FsPath),
}

pub type CommandRun = (Vec<String>, Vec<(String, String)>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppChoice {
    pub name: String,
    pub program: FsPath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenAsk {
    Script,
    NoApp { mime: String, apps: Vec<AppChoice> },
}

impl Launch {
    pub fn map_paths(&self, f: impl Fn(&FsPath) -> Option<FsPath>) -> Option<Launch> {
        let all = |v: &[FsPath]| v.iter().map(&f).collect::<Option<Vec<_>>>();
        Some(match self {
            Launch::Terminal { dirs, admin } => Launch::Terminal { dirs: all(dirs)?, admin: *admin },
            Launch::RunInTerminal { program } => Launch::RunInTerminal { program: f(program)? },
            Launch::With { program, args } => Launch::With { program: f(program)?, args: all(args)? },
            Launch::Run { program, args, dir } => Launch::Run { program: program.clone(), args: args.clone(), dir: dir.as_ref().map(&f).unwrap_or(None) },
            Launch::Eject(drive) => Launch::Eject(f(drive)?),
            Launch::Unmount(drive) => Launch::Unmount(f(drive)?),
            Launch::Mount(_) | Launch::Unlock { .. } => self.clone(),
            Launch::MapDrive(_) | Launch::Disconnect(_) | Launch::ConnectServer(_) | Launch::ConnectAndSaveServer(_) | Launch::ConnectNetwork { .. } => self.clone(),
            Launch::Reveal { item, new_tab } => Launch::Reveal { item: f(item)?, new_tab: *new_tab },
            Launch::ChooseApp { files, mime, apps } => {
                let apps = apps.iter().map(|a| Some(AppChoice { name: a.name.clone(), program: f(&a.program)? })).collect::<Option<Vec<_>>>()?;
                Launch::ChooseApp { files: all(files)?, mime: mime.clone(), apps }
            }
            Launch::SetDefault { program, mime } => Launch::SetDefault { program: f(program)?, mime: mime.clone() },
            Launch::Custom { command, files, mime, remember } => Launch::Custom { command: command.clone(), files: all(files)?, mime: mime.clone(), remember: *remember },
            Launch::Execute(program) => Launch::Execute(f(program)?),
            Launch::Display(file) => Launch::Display(f(file)?),
            Launch::Print(files) => Launch::Print(all(files)?),
            Launch::FileManager(dir) => Launch::FileManager(f(dir)?),
            Launch::Command { runs, dir } => Launch::Command { runs: runs.clone(), dir: dir.as_ref().map(&f).unwrap_or(None) },
            Launch::Wallpaper(image) => Launch::Wallpaper(f(image)?),
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
            FileOp::PasteLink { dest } => FileOp::PasteLink { dest: f(dest)? },
            FileOp::SetAccess { items, edit } => FileOp::SetAccess { items: all(items)?, edit: *edit },
            FileOp::Access(changes) => FileOp::Access(changes.iter().map(|c| Some(AccessChange { item: f(&c.item)?, ..c.clone() })).collect::<Option<_>>()?),
            FileOp::SetAttributes { items, set, clear, unblock } => FileOp::SetAttributes { items: all(items)?, set: *set, clear: *clear, unblock: *unblock },
            FileOp::PasteDropped { dest, moving, data } => FileOp::PasteDropped { dest: f(dest)?, moving: *moving, data: data.clone() },
            FileOp::GrantAccess(dir) => FileOp::GrantAccess(f(dir)?),
            FileOp::LinkAs { items, dest, kind } => FileOp::LinkAs { items: all(items)?, dest: f(dest)?, kind: *kind },
            FileOp::PasteLinkAs { dest, kind } => FileOp::PasteLinkAs { dest: f(dest)?, kind: *kind },
            FileOp::Segments { source, dest, part_size } => FileOp::Segments { source: f(source)?, dest: f(dest)?, part_size: *part_size },
            FileOp::Dedup { keep, candidates, backups } => FileOp::Dedup { keep: f(keep)?, candidates: all(candidates)?, backups: backups.iter().map(|b| b.map(&f)).collect::<Option<_>>()? },
            FileOp::RestoreDedup { keep, backups } => FileOp::RestoreDedup { keep: f(keep)?, backups: backups.iter().map(|b| b.map(&f)).collect::<Option<_>>()? },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Resolve {
    KeepBoth,
    Replace,
    Skip,
    SkipSame,
    ReplaceOlder,
}

const SAME_TIME: i64 = 20_000_000;

impl Resolve {
    pub fn decide(self, c: &Conflict) -> Option<Resolve> {
        let dir = |m: &Meta| m.attrs & crate::listing::attr::DIR != 0;
        match self {
            Resolve::KeepBoth | Resolve::Replace | Resolve::Skip => Some(self),
            _ if crate::limits::too_large(c) || crate::archive::is_unreadable(c) => Some(Resolve::Skip),
            _ if dir(&c.from) && dir(&c.to) => Some(Resolve::Replace),
            Resolve::SkipSame => c.identical().then_some(Resolve::Skip),
            Resolve::ReplaceOlder if dir(&c.from) || dir(&c.to) => Some(Resolve::Skip),
            Resolve::ReplaceOlder => Some(if c.from.modified > c.to.modified + SAME_TIME { Resolve::Replace } else { Resolve::Skip }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Conflict {
    pub source: FsPath,
    pub target: FsPath,
    pub from: Meta,
    pub to: Meta,
}

impl Conflict {
    pub fn identical(&self) -> bool {
        let file = |m: &Meta| m.attrs & crate::listing::attr::DIR == 0;
        file(&self.from) && file(&self.to) && self.from.size == self.to.size && self.from.modified.abs_diff(self.to.modified) <= SAME_TIME as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Recover {
    Retry,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Failure {
    pub path: FsPath,
    pub error: String,
    pub fatal: bool,
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
    skip_all: bool,
    recovered: Option<(Recover, bool)>,
}

#[derive(Default, Debug)]
struct Control {
    cancelled: AtomicBool,
    paused: AtomicBool,
    attach_companions: AtomicBool,
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

    pub fn attach_companions(&self) {
        self.0.attach_companions.store(true, Ordering::Release);
    }

    pub fn includes_companions(&self) -> bool {
        self.0.attach_companions.load(Ordering::Acquire)
    }

    pub fn pause(&self, paused: bool) {
        self.0.paused.store(paused, Ordering::Release);
        let _held = self.lock();
        self.0.changed.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.0.paused.load(Ordering::Acquire)
    }

    pub fn wait_while_paused(&self) {
        if !self.is_paused() {
            return;
        }
        let mut a = self.lock();
        while self.is_paused() && !self.is_cancelled() {
            a = self.0.changed.wait(a).unwrap_or_else(|e| e.into_inner());
        }
    }

    pub fn policy(&self) -> Option<Resolve> {
        self.lock().policy
    }

    pub fn answer(&self, choice: Resolve, all: bool) {
        self.lock().given = Some((choice, all));
        self.0.changed.notify_all();
    }

    pub fn settle(&self, choice: Resolve, policy: Option<Resolve>) {
        let mut a = self.lock();
        a.policy = policy.or(a.policy);
        a.given = Some((choice, false));
        self.0.changed.notify_all();
    }

    pub fn resolve(&self, c: &Conflict, mut ask: impl FnMut()) -> Option<Resolve> {
        loop {
            let mut a = self.lock();
            if let Some(choice) = a.policy.filter(|_| !crate::limits::is_short_of_space(c)).and_then(|p| p.decide(c)) {
                return Some(choice);
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
            if let Some(decided) = choice.decide(c) {
                return Some(decided);
            }
        }
    }

    pub fn skips_failures(&self) -> bool {
        self.lock().skip_all
    }

    pub fn answer_failure(&self, choice: Recover, all: bool) {
        self.lock().recovered = Some((choice, all));
        self.0.changed.notify_all();
    }

    pub fn recover(&self, fatal: bool, ask: impl FnOnce()) -> Option<Recover> {
        let mut a = self.lock();
        if a.skip_all && !fatal {
            return Some(Recover::Skip);
        }
        a.recovered = None;
        drop(a);
        if self.is_cancelled() {
            return None;
        }
        ask();
        let mut a = self.lock();
        while a.recovered.is_none() && !self.is_cancelled() {
            a = self.0.changed.wait(a).unwrap_or_else(|e| e.into_inner());
        }
        let (choice, all) = a.recovered.take().filter(|_| !self.is_cancelled())?;
        a.skip_all |= all && choice == Recover::Skip;
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
    DirSizes { tab: TabId, gen: u64, dir: FsPath, items: Vec<(u32, Vec<u8>)>, cancel: GenToken, allowed: [bool; 3] },
    Op { tab: TabId, id: OpId, op: FileOp, ctl: OpCtl },
    Open { tab: TabId, path: FsPath },
    Launch { tab: TabId, launch: Launch },
    Hash { tab: TabId, id: OpId, file: FsPath, kinds: Vec<HashKind>, ctl: OpCtl },
    Measure { tab: TabId, id: OpId, items: Vec<(FsPath, Option<u64>)>, ctl: OpCtl },
    Access { tab: TabId, id: OpId, items: Vec<FsPath> },
}

pub const ASK_PASSWORD: u32 = 1;
pub const ASK_USER: u32 = 2;
pub const ASK_DOMAIN: u32 = 4;
pub const ASK_SAVING: u32 = 8;
pub const ASK_ANONYMOUS: u32 = 16;

pub fn wipe(s: &mut String) {
    let bytes = unsafe { s.as_mut_vec() };
    for b in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(Ordering::SeqCst);
    bytes.clear();
}

#[derive(Default)]
pub struct Password(String);

impl Password {
    pub fn new(text: String) -> Self {
        Password(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginAsk {
    pub server: String,
    pub message: String,
    pub user: String,
    pub domain: String,
    pub flags: u32,
}

impl LoginAsk {
    pub fn wants(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountAsk {
    Login(LoginAsk),
    Question { server: String, message: String, choices: Vec<String> },
}

#[derive(Debug)]
pub enum MountReply {
    Login { user: String, domain: String, password: Password, anonymous: bool, remember: bool },
    Choice(usize),
    Cancel,
}

pub struct MountAnswer {
    id: u64,
    tx: std::sync::mpsc::Sender<MountReply>,
}

impl MountAnswer {
    pub fn new() -> (Self, std::sync::mpsc::Receiver<MountReply>) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let (tx, rx) = std::sync::mpsc::channel();
        (MountAnswer { id: NEXT.fetch_add(1, Ordering::Relaxed), tx }, rx)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn send(self, reply: MountReply) -> bool {
        self.tx.send(reply).is_ok()
    }
}

impl Clone for MountAnswer {
    fn clone(&self) -> Self {
        MountAnswer { id: self.id, tx: self.tx.clone() }
    }
}

impl PartialEq for MountAnswer {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl fmt::Debug for MountAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MountAnswer({})", self.id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    Changes { tab: TabId, gen: u64, changes: Vec<DirChange> },
    Rescan { tab: TabId, gen: u64 },
    DirSize { tab: TabId, gen: u64, idx: u32, stats: DirStats },
    SizesSkipped { tab: TabId, gen: u64, items: Vec<u32> },
    OpDone { tab: TabId, id: OpId, error: Option<String>, report: Option<OpReport> },
    OpProgress { tab: TabId, id: OpId, progress: OpProgress },
    OpNotice { tab: TabId, text: String },
    OpConflict { tab: TabId, id: OpId, conflict: Conflict },
    OpFailure { tab: TabId, id: OpId, failure: Failure },
    Untrashable { tab: TabId, id: OpId, items: Vec<FsPath> },
    Navigate { tab: TabId, path: FsPath },
    NetworkConnected { tab: TabId, gen: u64, result: Result<Option<FsPath>, String> },
    SavedServerConnected { tab: TabId, address: String, path: FsPath },
    Reveal { tab: TabId, path: FsPath, new_tab: bool },
    Hashed { tab: TabId, id: OpId, result: Result<Vec<String>, String> },
    Measured { tab: TabId, id: OpId, totals: Totals, file_system: Option<String>, done: bool },
    Status { tab: TabId, gen: u64, text: String },
    Unwatched { tab: TabId, gen: u64, limit: bool },
    Access { tab: TabId, id: OpId, items: Vec<Option<Access>>, accounts: Accounts },
    NotExecutable { tab: TabId, path: FsPath },
    WrongPassphrase { tab: TabId, device: FsPath },
    OpenAsk { tab: TabId, path: FsPath, ask: OpenAsk },
    MountAsk { tab: TabId, gen: Option<u64>, ask: MountAsk, answer: MountAnswer },
    MountAskDone { tab: TabId, id: u64 },
    Denied { tab: TabId, error: String },
}

#[cfg(test)]
#[path = "../test/provider.rs"]
mod tests;
