use crate::replay::FileSystem;
use flit_core::i18n::S;
use flit_core::undo::Stamp;
use flit_core::FsPath;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        LazyLock, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

const FORMAT: &[u8; 8] = b"FLITPD1\0";
const DELETE_FORMAT: &[u8; 8] = b"FLITPD2\0";
const DELETE_PREFIX: &[u8] = b".flit-deleting-";
const MAX_RECORD: u64 = 131_072;
const MAX_OBJECTS: usize = 100_000;
const MAX_INVENTORY: u64 = 64 << 20;
type Objects = HashMap<FsPath, (u64, u64)>;
type RecoveryReporter = Box<dyn Fn(String) + Send + Sync>;

static RECOVERY_REPORTER: OnceLock<RecoveryReporter> = OnceLock::new();

pub fn set_recovery_reporter(report: impl Fn(String) + Send + Sync + 'static) {
    let _ = RECOVERY_REPORTER.set(Box::new(report));
}

pub fn is_stage_name(name: &[u8]) -> bool {
    (name.starts_with(DELETE_PREFIX) && active_deletes().contains(name)) || (name.starts_with(b".flit-stage-") && name.ends_with(b".flit-partial"))
}

pub fn is_delete_name(name: &[u8]) -> bool {
    flit_core::name::is_delete_stage(name)
}

fn active_deletes() -> std::sync::MutexGuard<'static, HashSet<Vec<u8>>> {
    static ACTIVE: LazyLock<Mutex<HashSet<Vec<u8>>>> = LazyLock::new(Mutex::default);
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

static NEXT: AtomicU64 = AtomicU64::new(0);

fn id() -> String {
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{}-{time:x}-{n:x}", std::process::id())
}

#[derive(Debug)]
pub struct Record {
    log: PathBuf,
    identity: (u64, u64),
    active: Option<Vec<u8>>,
}

impl Drop for Record {
    fn drop(&mut self) {
        if let Some(name) = &self.active {
            active_deletes().remove(name);
        }
    }
}

impl Record {
    pub fn finish(self) {
        self.keep();
        let _ = fs::remove_file(self.log.with_extension("objects"));
        match fs::remove_file(&self.log) {
            Ok(()) => {
                let _ = fs::remove_file(self.log.with_extension("restore"));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(?error, record = ?self.log, "Flit could not remove a pending-delete record"),
        }
    }

    pub fn matches(&self, stamp: Stamp) -> bool {
        self.identity == (stamp.device, stamp.file)
    }

    pub fn begin_delete(&self) -> io::Result<()> {
        fs::File::create_new(self.log.with_extension("delete"))?.sync_all()
    }

    pub fn keep(&self) {
        if let Err(error) = self.disarm() {
            let restore = self.log.with_extension("restore");
            if let Err(mark_error) = fs::File::create_new(&restore).and_then(|file| file.sync_all()) {
                if !restore.is_file() {
                    tracing::warn!(?error, ?mark_error, record = ?self.log, "Flit could not disarm a pending delete");
                }
            }
        }
    }

    fn disarm(&self) -> io::Result<()> {
        match fs::remove_file(self.log.with_extension("delete")) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    pub fn inventory(&self, path: &FsPath, file_system: FileSystem, cancelled: impl Fn() -> bool) -> io::Result<()> {
        let entries = objects(path, file_system, cancelled)?;
        if entries.get(path) != Some(&self.identity) {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        let manifest = self.log.with_extension("objects");
        let mut writer = io::BufWriter::new(fs::File::create_new(manifest)?);
        writer.write_all(&(entries.len() as u32).to_le_bytes())?;
        for (path, (device, file)) in entries {
            writer.write_all(&(path.as_bytes().len() as u32).to_le_bytes())?;
            writer.write_all(path.as_bytes())?;
            writer.write_all(&device.to_le_bytes())?;
            writer.write_all(&file.to_le_bytes())?;
        }
        writer.flush()?;
        writer.get_ref().sync_all()
    }
}

fn objects(path: &FsPath, fs: FileSystem, cancelled: impl Fn() -> bool) -> io::Result<Objects> {
    let mut entries = HashMap::new();
    let mut paths = vec![path.clone()];
    let mut bytes = 0u64;
    let mut device = None;
    while let Some(path) = paths.pop() {
        bytes += path.as_bytes().len() as u64 + 20;
        if entries.len() >= MAX_OBJECTS || bytes >= MAX_INVENTORY || cancelled() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let (stamp, directory) = (fs.stamp)(&path)?;
        if *device.get_or_insert(stamp.device) != stamp.device {
            return Err(io::Error::other("the staged tree crosses a mount point"));
        }
        if directory {
            paths.extend((fs.children)(&path)?);
        }
        entries.insert(path, (stamp.device, stamp.file));
    }
    Ok(entries)
}

fn recorded_objects(log: &Path) -> Option<Objects> {
    let mut bytes = Vec::new();
    fs::File::open(log.with_extension("objects")).ok()?.take(MAX_INVENTORY + 4).read_to_end(&mut bytes).ok()?;
    let count = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    if count == 0 || count > MAX_OBJECTS || bytes.len() as u64 >= MAX_INVENTORY + 4 {
        return None;
    }
    let mut data = bytes.get(4..)?;
    let mut entries = HashMap::with_capacity(count);
    for _ in 0..count {
        let len = u32::from_le_bytes(data.get(..4)?.try_into().ok()?) as usize;
        let end = len.checked_add(4)?;
        let path = FsPath::parse_exact(data.get(4..end)?)?;
        data = data.get(end..)?;
        let device = u64::from_le_bytes(data.get(..8)?.try_into().ok()?);
        let file = u64::from_le_bytes(data.get(8..16)?.try_into().ok()?);
        data = data.get(16..)?;
        if entries.insert(path, (device, file)).is_some() {
            return None;
        }
    }
    data.is_empty().then_some(entries)
}

fn remember(state: &Path, id: &str, path: &FsPath, stamp: Stamp, hidden: Option<bool>) -> io::Result<Record> {
    let dir = state.join("pending-deletes");
    fs::create_dir_all(&dir)?;
    let log = dir.join(id);
    let mut file = fs::File::create_new(&log)?;
    let result = (|| {
        file.write_all(if hidden.is_some() { DELETE_FORMAT } else { FORMAT })?;
        file.write_all(&stamp.device.to_le_bytes())?;
        file.write_all(&stamp.file.to_le_bytes())?;
        if let Some(hidden) = hidden {
            file.write_all(&[u8::from(hidden)])?;
        }
        file.write_all(path.as_bytes())?;
        file.sync_all()
    })();
    drop(file);
    let record = Record { log, identity: (stamp.device, stamp.file), active: None };
    match result {
        Ok(()) => Ok(record),
        Err(error) => {
            record.finish();
            Err(error)
        }
    }
}

pub fn stage_delete(state: &Path, original: &FsPath, stamp: Stamp, hidden: bool, rename: impl Fn(&FsPath, &FsPath) -> io::Result<()>) -> io::Result<(FsPath, Record)> {
    let parent = original.parent().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let id = id();
    let staged = parent.join(&[DELETE_PREFIX, id.as_bytes()].concat());
    let mut record = remember(state, &id, original, stamp, Some(hidden))?;
    let name = staged.file_name().to_vec();
    active_deletes().insert(name.clone());
    record.active = Some(name);
    match rename(original, &staged) {
        Ok(()) => Ok((staged, record)),
        Err(error) => {
            record.finish();
            Err(error)
        }
    }
}

pub fn partial(state: &Path, staged: &FsPath, stamp: Stamp) -> io::Result<Record> {
    if !staged.file_name().ends_with(b".flit-partial") {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    remember(state, &id(), staged, stamp, None)
}

struct Saved {
    staged: FsPath,
    identity: (u64, u64),
    original: Option<(FsPath, bool)>,
}

fn read(path: &Path, name: &str) -> Option<Saved> {
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(fs::File::open(path).ok()?, MAX_RECORD), &mut bytes).ok()?;
    if bytes.len() as u64 == MAX_RECORD {
        return None;
    }
    let device = u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?);
    let file = u64::from_le_bytes(bytes.get(16..24)?.try_into().ok()?);
    let identity = (device, file);
    let format = bytes.get(..8)?;
    if format == DELETE_FORMAT {
        let hidden = match *bytes.get(24)? {
            0 => false,
            1 => true,
            _ => return None,
        };
        let original = FsPath::parse_exact(bytes.get(25..)?)?;
        let staged = original.parent()?.join(&[DELETE_PREFIX, name.as_bytes()].concat());
        return (!staged.same(&original) && is_delete_name(staged.file_name())).then_some(Saved { staged, identity, original: Some((original, hidden)) });
    }
    if format != FORMAT {
        return None;
    }
    let path = FsPath::parse_exact(bytes.get(24..)?)?;
    let leaf = path.file_name();
    if leaf.strip_prefix(DELETE_PREFIX) != Some(name.as_bytes()) && !leaf.ends_with(b".flit-partial") {
        return None;
    }
    Some(Saved { staged: path, identity, original: None })
}

pub fn recover(state: &Path, live: impl Fn(u32) -> bool, fs: FileSystem, remove: impl Fn(&FsPath) -> io::Result<()>, restore: impl Fn(&FsPath, &FsPath, bool) -> io::Result<()>, reveal: impl Fn(&FsPath) -> io::Result<()>) {
    let entries = match fs::read_dir(state.join("pending-deletes")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(?error, "Flit could not read the pending-delete directory");
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.contains('.') {
            continue;
        }
        let Some(pid) = name.split_once('-').and_then(|(pid, _)| pid.parse::<u32>().ok()).filter(|&pid| pid != 0) else { continue };
        if live(pid) {
            continue;
        }
        let log = entry.path();
        let Some(Saved { staged: path, identity, original }) = read(&log, name) else {
            tracing::warn!(?log, "Flit could not read a pending-delete record");
            continue;
        };
        let record = Record { log, identity, active: None };
        let deleting = path.file_name().starts_with(DELETE_PREFIX);
        let result = match (fs.stamp)(&path) {
            Ok((stamp, _)) if !record.matches(stamp) => {
                if deleting {
                    Err(io::Error::from(io::ErrorKind::WouldBlock))
                } else {
                    Ok(())
                }
            }
            Ok((_, directory)) => {
                let armed = record.log.with_extension("delete").is_file() && !record.log.with_extension("restore").is_file();
                match &original {
                    Some((original, hidden)) if !armed => restore(&path, original, *hidden),
                    _ if deleting && directory => remove_tree(&record, &path, fs, &remove),
                    Some(_) => remove(&path),
                    None if deleting => Err(io::Error::from(io::ErrorKind::Unsupported)),
                    None => remove(&path),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => record.finish(),
            Err(error) => {
                if deleting {
                    if let Err(error) = reveal(&path) {
                        tracing::warn!(?error, path = %path.display(), "Flit could not show an item left by a pending delete");
                    }
                }
                tracing::warn!(?error, path = %path.display(), original = ?original.as_ref().map(|(path, _)| path.display()), "Flit could not recover a pending delete");
                if let (Some((original, _)), Some(report)) = (original, RECOVERY_REPORTER.get()) {
                    report(flit_core::tr_en!(S::ErrorsOpsSavedFile, original = original.display(), saved = path.display(), error = flit_core::fmt::io_reason(&error)));
                }
            }
        }
    }
}

fn remove_tree(record: &Record, path: &FsPath, fs: FileSystem, remove: &impl Fn(&FsPath) -> io::Result<()>) -> io::Result<()> {
    let changed = || io::Error::other("the staged tree contains objects outside its original inventory");
    let before = recorded_objects(&record.log).ok_or_else(changed)?;
    let now = objects(path, fs, || false)?;
    if now.get(path) != Some(&record.identity) || now.iter().any(|(path, id)| before.get(path) != Some(id)) {
        return Err(changed());
    }
    let mut entries: Vec<_> = now.into_iter().collect();
    entries.sort_unstable_by_key(|(path, _)| std::cmp::Reverse(path.as_bytes().len()));
    for (path, identity) in entries {
        match (fs.stamp)(&path) {
            Ok((stamp, _)) if identity == (stamp.device, stamp.file) => remove(&path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(changed()),
        }
    }
    Ok(())
}
