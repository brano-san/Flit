use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use flit_core::hash::FxHashMap;
use flit_core::open::{self, Open};
use flit_core::i18n::S;
use flit_core::tags::{self, FileKey, TagCell, TagEvent, TagRequest, TagStore, Tagged, Tags, COLORS};
use flit_core::ui::{SearchKey, SearchRow, SearchSort};
use flit_core::{FsPath, Meta};
use flit_platform::{Platform, PlatformError, Resolved, Wake, WindowId};
use flit_search::index::lowercase;
use flit_search::query::{Filter, Plain, Query};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const FILE_VERSION: u32 = 1;
const RECHECK_EVERY: Duration = Duration::from_secs(120);

#[derive(Serialize, Deserialize)]
struct TagFile {
    version: u32,
    store: TagStore,
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub changed: bool,
    pub files: Vec<(FsPath, u32)>,
    pub dropped: Option<usize>,
    pub status: Option<String>,
}

pub struct Book {
    pub store: TagStore,
    seen: FxHashMap<FileKey, (Meta, bool)>,
    version: u64,
    file: Option<PathBuf>,
    key_of: fn(&FsPath) -> Option<FileKey>,
    resolve: fn(&[FileKey]) -> Vec<Resolved>,
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn moved_path(path: &FsPath, from: &FsPath, to: &FsPath) -> Option<FsPath> {
    let (p, f) = (path.as_bytes(), from.as_bytes());
    if same(p, f) {
        return Some(to.clone());
    }
    let rest = p.get(f.len()..).filter(|r| same(&p[..f.len()], f) && r.first() == Some(&path.sep()))?;
    FsPath::parse_bytes(&[to.as_bytes(), rest].concat())
}

impl Book {
    pub fn load(file: Option<PathBuf>, key_of: fn(&FsPath) -> Option<FileKey>, resolve: fn(&[FileKey]) -> Vec<Resolved>) -> Self {
        let bytes = file.as_ref().and_then(|f| std::fs::read(f).ok());
        let store = match bytes.as_deref().map(postcard::from_bytes::<TagFile>) {
            None => TagStore::default(),
            Some(Ok(t)) if t.version == FILE_VERSION => t.store,
            Some(_) => {
                if let Some(f) = &file {
                    tracing::warn!("{} is not readable; it is kept as tags.bin.bad", f.display());
                    let _ = std::fs::rename(f, f.with_extension("bin.bad"));
                }
                TagStore::default()
            }
        };
        Self { store, seen: FxHashMap::default(), version: 0, file, key_of, resolve }
    }

    fn save(&self) {
        let Some(f) = &self.file else { return };
        let Ok(bytes) = postcard::to_stdvec(&TagFile { version: FILE_VERSION, store: self.store.clone() }) else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = f.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, f).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    pub fn publish(&mut self) -> Arc<Tags> {
        self.version += 1;
        Arc::new(Tags::new(self.version, &self.store, &self.seen))
    }

    fn learn(&mut self, keys: Vec<FileKey>) -> bool {
        let found = (self.resolve)(&keys);
        let mut changed = false;
        for (k, r) in keys.into_iter().zip(found) {
            let seen = match r {
                Resolved::Found(path, meta) => {
                    if let Some(t) = self.store.files.get_mut(&k).filter(|t| t.path != path) {
                        t.path = path;
                        changed = true;
                    }
                    (meta, true)
                }
                Resolved::Gone => {
                    changed |= self.store.files.remove(&k).is_some();
                    self.seen.remove(&k);
                    continue;
                }
                Resolved::Unavailable => (Meta::default(), false),
            };
            changed |= self.seen.insert(k, seen) != Some(seen);
        }
        changed
    }

    fn learn_new(&mut self) {
        let fresh: Vec<FileKey> = self.store.files.keys().filter(|k| !self.seen.contains_key(k)).copied().collect();
        if !fresh.is_empty() {
            self.learn(fresh);
        }
    }

    pub fn recheck(&mut self) -> bool {
        let all: Vec<FileKey> = self.store.files.keys().copied().collect();
        let changed = !all.is_empty() && self.learn(all);
        if changed {
            self.save();
        }
        changed
    }

    fn set(&mut self, items: Vec<FsPath>, tag: usize, on: bool, out: &mut Outcome) {
        if tag >= self.store.tags.len() {
            out.status = Some(S::ErrorsTagsGone.en().into());
            return;
        }
        let mut refused = 0;
        for item in items {
            let Some(key) = (self.key_of)(&item) else {
                refused += 1;
                continue;
            };
            let mask = self.store.update(key, item.clone(), |m| if on { m | 1 << tag } else { m & !(1 << tag) });
            out.files.push((item, mask));
            out.changed = true;
        }
        if refused > 0 {
            out.status = Some(S::ErrorsTagsNotTaggable.en().into());
        }
    }

    fn moved(&mut self, pairs: Vec<(FsPath, FsPath)>, copy: bool, out: &mut Outcome) {
        for (from, to) in pairs {
            let hits: Vec<(FileKey, Tagged)> = self.store.files.iter().filter_map(|(k, t)| Some((*k, Tagged { mask: t.mask, path: moved_path(&t.path, &from, &to)? }))).collect();
            out.changed |= !hits.is_empty();
            for (key, t) in hits {
                let now = (self.key_of)(&t.path);
                if !copy && now != Some(key) {
                    self.store.files.remove(&key);
                    self.seen.remove(&key);
                }
                let Some(now) = now else { continue };
                let mask = self.store.update(now, t.path.clone(), |m| m | t.mask);
                out.files.push((t.path, mask));
            }
        }
    }

    pub fn apply(&mut self, req: TagRequest) -> Outcome {
        let mut out = Outcome::default();
        match req {
            TagRequest::Set { items, tag, on } => self.set(items, tag, on, &mut out),
            TagRequest::Clear(items) => {
                for item in items {
                    if let Some(key) = (self.key_of)(&item).filter(|k| self.store.files.contains_key(k)) {
                        self.store.update(key, item.clone(), |_| 0);
                        out.files.push((item, 0));
                        out.changed = true;
                    }
                }
            }
            TagRequest::Create { name, color, assign } => match self.store.add(&name, color) {
                Ok(k) => {
                    out.changed = true;
                    self.set(assign, k, true, &mut out);
                }
                Err(e) => out.status = Some(e.into()),
            },
            TagRequest::Rename { tag, name } => match self.store.rename(tag, &name) {
                Ok(()) => out.changed = true,
                Err(e) => out.status = Some(e.into()),
            },
            TagRequest::Recolor { tag, color } => {
                self.store.recolor(tag, color);
                out.changed = true;
            }
            TagRequest::Delete(k) => {
                if self.store.delete(k) {
                    self.seen.retain(|key, _| self.store.files.contains_key(key));
                    out.changed = true;
                    out.dropped = Some(k);
                }
            }
            TagRequest::Moved { pairs, copy } => self.moved(pairs, copy, &mut out),
            TagRequest::Command { name, items } => self.command(&name, items, &mut out),
        }
        if out.changed {
            self.learn_new();
            self.save();
        }
        out
    }

    fn command(&mut self, name: &str, items: Vec<FsPath>, out: &mut Outcome) {
        let k = match tags::find(&self.store.tags, name) {
            Some(k) => k,
            None => match self.store.add(name, COLORS[self.store.tags.len() % COLORS.len()].1) {
                Ok(k) => k,
                Err(e) => return out.status = Some(e.into()),
            },
        };
        out.changed = true;
        let n = items.len();
        self.set(items, k, true, out);
        if out.status.is_none() {
            out.status = Some(flit_core::trn!(flit_core::i18n::P::SidebarTagTagged, n, tag = self.store.tags[k].name));
        }
    }
}

fn state_file<P: Platform>() -> Option<PathBuf> {
    P::state_dir().map(|d| d.join("tags.bin"))
}

pub fn command_items<P: Platform>(o: &Open) -> Vec<FsPath> {
    o.paths.iter().filter_map(|raw| open::resolve(raw, o.cwd.as_deref(), &P::known_folder).ok()?.to_windows()).collect()
}

pub fn run_command<P: Platform>(o: &Open) -> Result<String, String> {
    let mut book = Book::load(state_file::<P>(), P::file_key, P::resolve_files);
    let items = command_items::<P>(o);
    if items.is_empty() {
        return Err(S::SidebarTagNeedsArgs.t().into());
    }
    let out = book.apply(TagRequest::Command { name: o.tag.clone().unwrap_or_default(), items });
    let status = out.status.unwrap_or_default();
    if out.files.is_empty() { Err(status) } else { Ok(status) }
}

pub struct TagWorker {
    tx: Sender<(Option<WindowId>, TagRequest)>,
    pub events: Receiver<(Option<WindowId>, TagEvent)>,
    cell: TagCell,
}

impl TagWorker {
    pub fn start<P: Platform>(cell: TagCell, wake: Wake) -> Result<Self, PlatformError> {
        let mut book = Book::load(state_file::<P>(), P::file_key, P::resolve_files);
        cell.set(book.publish());
        let (tx, rx) = unbounded::<(Option<WindowId>, TagRequest)>();
        let (out, events) = unbounded();
        let shared = cell.clone();
        let run = move || {
            let publish = |book: &mut Book, files: Vec<(FsPath, u32)>, dropped: Option<usize>| {
                let tags = book.publish();
                shared.set(tags.clone());
                out.send((None, TagEvent::Changed { tags, files, dropped })).is_ok()
            };
            if book.recheck() && !publish(&mut book, Vec::new(), None) {
                return;
            }
            wake();
            loop {
                match rx.recv_timeout(RECHECK_EVERY) {
                    Ok((window, req)) => {
                        let o = book.apply(req);
                        if o.changed && !publish(&mut book, o.files, o.dropped) {
                            return;
                        }
                        if let Some(s) = o.status {
                            let _ = out.send((window, TagEvent::Status(s)));
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if !book.recheck() {
                            continue;
                        }
                        if !publish(&mut book, Vec::new(), None) {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                wake();
            }
        };
        std::thread::Builder::new().name("flit-tags".into()).spawn(run).map_err(|e| PlatformError::new(-1, format!("cannot start flit-tags: {e}")))?;
        Ok(Self { tx, events, cell })
    }

    pub fn submit(&self, window: Option<WindowId>, req: TagRequest) {
        let _ = self.tx.send((window, req));
    }

    pub fn current(&self) -> Arc<Tags> {
        self.cell.get()
    }
}

pub fn search(t: &Tags, q: &Query, sort: SearchSort) -> Result<Vec<SearchRow>, String> {
    let mut wanted: FxHashMap<&str, u32> = FxHashMap::default();
    for term in &q.terms {
        if let Filter::Tag(name) = &term.filter {
            let bits = match tags::find(&t.defs, name) {
                _ if name.is_empty() => u32::MAX,
                Some(k) => 1 << k,
                None => return Err(flit_core::tr!(S::ErrorsTagsNoTag, name = name)),
            };
            wanted.insert(name, bits);
        }
    }
    let mut rows: Vec<(Vec<u8>, Vec<u8>, SearchRow)> = Vec::new();
    for f in t.files.iter().filter(|f| f.present) {
        let (path, name) = (f.path.as_bytes(), f.path.file_name());
        let (lower_path, lower_name) = (lowercase(path), lowercase(name));
        let plain = Plain { name, lower_name: &lower_name, path, lower_path: &lower_path, size: f.meta.size, modified: f.meta.modified, dir: f.meta.attrs & flit_core::attr::DIR != 0 };
        if q.matches(&plain, &|n| wanted.get(n).is_some_and(|&b| f.mask & b != 0)) {
            let dir = f.path.parent().map(|p| p.as_bytes().to_vec()).unwrap_or_default();
            rows.push((lower_name, lowercase(&dir), SearchRow { name: name.to_vec(), dir, meta: f.meta }));
        }
    }
    match sort.key {
        SearchKey::Name => rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1))),
        SearchKey::Folder => rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0))),
        SearchKey::Size => rows.sort_by(|a, b| a.2.meta.size.cmp(&b.2.meta.size).then_with(|| a.0.cmp(&b.0))),
        SearchKey::Modified => rows.sort_by(|a, b| a.2.meta.modified.cmp(&b.2.meta.modified).then_with(|| a.0.cmp(&b.0))),
    }
    if sort.descending {
        rows.reverse();
    }
    Ok(rows.into_iter().map(|r| r.2).collect())
}

#[cfg(test)]
#[path = "../test/tags.rs"]
mod tests;
