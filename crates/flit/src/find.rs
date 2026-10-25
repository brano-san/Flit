use flit_core::listing::ListingBuilder;
use flit_core::sort::CaseMode;
use flit_core::ui::{SearchKey, SearchRow, SearchSort};
use flit_core::{attr, FsPath};
use flit_platform::Fs;
use flit_search::index::lowercase;
use flit_search::query::{Plain, Query};
use std::cmp::Ordering;
use std::sync::Mutex;

pub type KeepGoing<'a> = &'a (dyn Fn() -> bool + Sync);

pub struct Hit {
    lower_name: Vec<u8>,
    lower_dir: Vec<u8>,
    pub row: SearchRow,
}

impl Hit {
    pub fn new(row: SearchRow) -> Self {
        Self { lower_name: lowercase(&row.name), lower_dir: lowercase(&row.dir), row }
    }

    pub fn within(&self, lower_folder: &[u8]) -> bool {
        let d = &self.lower_dir;
        d.starts_with(lower_folder) && (d.len() == lower_folder.len() || lower_folder.ends_with(b"\\") || lower_folder.ends_with(b"/") || matches!(d[lower_folder.len()], b'\\' | b'/'))
    }
}

#[derive(Default)]
pub struct Found {
    pub hits: Vec<Hit>,
    pub folders: u64,
    pub done: bool,
    pub error: Option<String>,
}

fn compare(a: &Hit, b: &Hit, key: SearchKey) -> Ordering {
    let by_name = || a.lower_name.cmp(&b.lower_name).then_with(|| a.lower_dir.cmp(&b.lower_dir));
    match key {
        SearchKey::Name => by_name(),
        SearchKey::Folder => a.lower_dir.cmp(&b.lower_dir).then_with(|| a.lower_name.cmp(&b.lower_name)),
        SearchKey::Size => a.row.meta.size.cmp(&b.row.meta.size).then_with(by_name),
        SearchKey::Modified => a.row.meta.modified.cmp(&b.row.meta.modified).then_with(by_name),
    }
}

pub fn sort(hits: &mut [Hit], s: SearchSort) {
    if s.descending {
        hits.sort_by(|a, b| compare(b, a, s.key));
    } else {
        hits.sort_by(|a, b| compare(a, b, s.key));
    }
}

pub fn groups(hits: &[Hit]) -> Vec<u64> {
    if hits.is_empty() {
        return Vec::new();
    }
    let mut starts = vec![0];
    starts.extend(hits.windows(2).enumerate().filter(|(_, w)| w[0].lower_dir != w[1].lower_dir).map(|(k, _)| k as u64 + 1));
    starts
}

fn visit<F: Fs>(fs: &F, dir: &FsPath, query: &Query, keep_going: KeepGoing<'_>, found: &Mutex<Found>) -> Vec<FsPath> {
    let mut on_batch = |_: &ListingBuilder<'_>| keep_going();
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut on_batch);
    let read = fs.read_dir(dir, &mut b);
    let l = std::mem::take(&mut b.listing);
    drop(b);
    if read.is_err() || !keep_going() {
        return Vec::new();
    }
    let needs_path = query.needs_path();
    let (mut subdirs, mut hits) = (Vec::new(), Vec::new());
    let mut path = Vec::new();
    for i in 0..l.len() as u32 {
        let e = &l.entries[i as usize];
        let name = l.name(i);
        let lower_name = lowercase(name);
        let lower_path = if needs_path {
            path.clear();
            path.extend_from_slice(dir.join(name).as_bytes());
            lowercase(&path)
        } else {
            Vec::new()
        };
        let plain = Plain { name, lower_name: &lower_name, path: &path, lower_path: &lower_path, size: e.size, modified: e.modified, dir: e.is_dir() };
        if query.matches(&plain, &|_| false) {
            let row = SearchRow { name: name.to_vec(), dir: dir.as_bytes().to_vec(), meta: e.meta() };
            hits.push(Hit { lower_name, lower_dir: lowercase(dir.as_bytes()), row });
        }
        if e.is_dir() && e.attrs & attr::LINK == 0 {
            subdirs.push(dir.join(name));
        }
    }
    let mut f = found.lock().unwrap_or_else(|e| e.into_inner());
    f.hits.append(&mut hits);
    f.folders += 1;
    subdirs
}

fn spawn<'s, F: Fs>(s: &rayon::Scope<'s>, fs: &'s F, dir: FsPath, query: &'s Query, keep_going: KeepGoing<'s>, found: &'s Mutex<Found>) {
    s.spawn(move |s| {
        for sub in visit(fs, &dir, query, keep_going, found) {
            spawn(s, fs, sub, query, keep_going, found);
        }
    });
}

pub fn walk<F: Fs>(fs: &F, root: &FsPath, query: &Query, keep_going: KeepGoing<'_>, found: &Mutex<Found>) {
    if fs.seek_penalty(root) == Some(true) {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop().filter(|_| keep_going()) {
            stack.extend(visit(fs, &dir, query, keep_going, found));
        }
    } else {
        rayon::scope(|s| spawn(s, fs, root.clone(), query, keep_going, found));
    }
}

#[cfg(test)]
#[path = "../test/find.rs"]
mod tests;
