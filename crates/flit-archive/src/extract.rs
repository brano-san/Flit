use crate::index::{self, rar_error, sevenz_error, Index, Item};
use crate::names::fold;
use crate::{corrupt, io_error, password_error, std_path, Env};
use flit_core::archive::{self, Format, Place, Secret};
use flit_core::listing::{attr, Meta};
use flit_core::path::FsPath;
use flit_core::provider::{Conflict, Resolve, Stage};
use flit_platform::{Job, PlatformError, CANCELLED};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BOMB_RATIO: u64 = 10_000;
const BOMB_FLOOR: u64 = 1 << 30;
const CHUNK: usize = 256 * 1024;
const DIRECT: u64 = 1 << 20;

pub type Pairs = Vec<(FsPath, FsPath)>;
pub type Outcome = Result<Pairs, (Pairs, PlatformError)>;

static PART: AtomicU32 = AtomicU32::new(0);

pub fn next_part() -> u32 {
    PART.fetch_add(1, Ordering::Relaxed)
}

pub fn bomb() -> PlatformError {
    PlatformError::new(-1, "Extraction stopped: the archive expands far beyond its size and looks like a decompression bomb")
}

pub fn cancelled() -> PlatformError {
    PlatformError::new(-1, CANCELLED)
}

pub fn system_time(ticks: i64) -> Option<SystemTime> {
    let secs = ticks / 10_000_000 - 11_644_473_600;
    (ticks > 0 && secs > 0).then(|| UNIX_EPOCH + Duration::from_secs(secs as u64) + Duration::from_nanos((ticks % 10_000_000) as u64 * 100))
}

pub fn ticks(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| (d.as_secs() as i64 + 11_644_473_600) * 10_000_000 + d.subsec_nanos() as i64 / 100)
}

pub fn meta_of(p: &Path) -> Option<Meta> {
    let m = fs::symlink_metadata(p).ok()?;
    let attrs = if m.is_dir() { attr::DIR } else { 0 };
    Some(Meta { size: m.len(), modified: m.modified().map_or(0, ticks), created: 0, attrs })
}

pub fn numbered_by(name: &str, is_dir: bool, taken: impl Fn(&str) -> bool) -> String {
    let ext = if is_dir { 0 } else { flit_core::name::ext_len(name.as_bytes()) };
    let (stem, ext) = if ext == 0 || ext + 1 >= name.len() { (name, "") } else { name.split_at(name.len() - ext - 1) };
    (2u32..).map(|k| format!("{stem} ({k}){ext}")).find(|n| !taken(n)).unwrap_or_default()
}

pub fn numbered(dir: &Path, name: &str, is_dir: bool) -> String {
    numbered_by(name, is_dir, |n| dir.join(n).exists())
}

fn first(rel: &str) -> &str {
    rel.split('/').next().unwrap_or(rel)
}

fn existing_ancestor(p: &Path) -> Option<&Path> {
    p.ancestors().find(|a| a.exists())
}

struct Plan<'a> {
    archive: &'a FsPath,
    index: &'a Index,
    steps: Vec<(usize, String)>,
    target: FsPath,
}

impl Plan<'_> {
    fn declared(&self) -> u64 {
        self.steps.iter().map(|&(i, _)| self.index.items[i].size).sum()
    }
}

struct Out<'a> {
    job: &'a Job,
    target: PathBuf,
    target_fs: FsPath,
    archive: &'a FsPath,
    renamed: HashMap<String, String>,
    cap: u64,
    free: u64,
    written: u64,
    skipped: Vec<String>,
    made: HashSet<PathBuf>,
}

impl Out<'_> {
    fn dest(&self, rel: &str) -> (String, PathBuf, FsPath) {
        let root = first(rel);
        let rel = match self.renamed.get(root) {
            Some(new) => format!("{new}{}", &rel[root.len()..]),
            None => rel.to_owned(),
        };
        let mut p = self.target.clone();
        let mut f = self.target_fs.clone();
        for c in rel.split('/') {
            p.push(c);
            f = f.join(c.as_bytes());
        }
        (rel, p, f)
    }

    fn dir(&mut self, rel: &str) -> Result<(), PlatformError> {
        let (_, p, _) = self.dest(rel);
        fs::create_dir_all(&p).map_err(io_error)?;
        self.job.update(|pr| pr.items += 1);
        Ok(())
    }

    fn prepare(&mut self, item: &Item, rel: &str) -> Result<Option<(PathBuf, bool)>, PlatformError> {
        let (rel, dest, dest_fs) = self.dest(rel);
        let parent = dest.parent().map(Path::to_path_buf).unwrap_or_else(|| self.target.clone());
        if !self.made.contains(&parent) {
            fs::create_dir_all(&parent).map_err(io_error)?;
            self.made.insert(parent.clone());
        }
        let Some(existing) = meta_of(&dest) else { return Ok(Some((dest, true))) };
        if existing.attrs & attr::DIR != 0 {
            self.skipped.push(rel);
            return Ok(None);
        }
        let from = Meta { size: item.size, modified: item.modified, created: 0, attrs: 0 };
        let source = self.archive.join(item.path.as_bytes());
        match self.job.resolve(Conflict { source, target: dest_fs, from, to: existing }) {
            Some(Resolve::Replace) => Ok(Some((dest, false))),
            Some(Resolve::KeepBoth) => {
                let name = numbered(&parent, dest.file_name().and_then(|n| n.to_str()).unwrap_or("file"), false);
                if !rel.contains('/') {
                    self.renamed.insert(rel, name.clone());
                }
                Ok(Some((parent.join(name), false)))
            }
            Some(Resolve::Skip) => {
                self.job.update(|p| {
                    p.items += 1;
                    p.bytes += item.size;
                });
                Ok(None)
            }
            None => Err(cancelled()),
        }
    }

    fn part(dest: &Path) -> PathBuf {
        dest.with_file_name(format!("~{}-{}.flit-part", std::process::id(), PART.fetch_add(1, Ordering::Relaxed)))
    }

    fn commit(&self, part: &Path, dest: &Path, result: Result<(), PlatformError>) -> Result<(), PlatformError> {
        let result = result.and_then(|()| if part == dest { Ok(()) } else { fs::rename(part, dest).map_err(io_error) });
        if result.is_err() {
            let _ = fs::remove_file(part);
        }
        result?;
        self.job.update(|p| p.items += 1);
        Ok(())
    }

    fn file(&mut self, item: &Item, rel: &str, declared: bool, reader: &mut dyn Read) -> Result<(), PlatformError> {
        let Some((dest, fresh)) = self.prepare(item, rel)? else { return Ok(()) };
        let part = if fresh && declared && item.size <= DIRECT { dest.clone() } else { Self::part(&dest) };
        let result = self.write(&part, item, declared, reader);
        self.commit(&part, &dest, result)
    }

    fn write(&mut self, part: &Path, item: &Item, declared: bool, reader: &mut dyn Read) -> Result<(), PlatformError> {
        let mut f = File::create_new(part).map_err(io_error)?;
        let mut buf = vec![0u8; CHUNK];
        let mut total = 0u64;
        loop {
            if self.job.cancelled() {
                return Err(cancelled());
            }
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(read_error(e)),
            };
            total += n as u64;
            self.written += n as u64;
            if !declared && self.written > self.free {
                return Err(no_space(self.written, self.free));
            }
            if (declared && total > item.size) || (!declared && self.written > self.cap) {
                return Err(bomb());
            }
            f.write_all(&buf[..n]).map_err(io_error)?;
            self.job.update(|p| p.bytes += n as u64);
        }
        stamp(&f, item.modified);
        Ok(())
    }
}

fn stamp(f: &File, modified: i64) {
    if let Some(t) = system_time(modified) {
        let _ = f.set_modified(t);
    }
}

const DAMAGED: &str = "The archive data is damaged";

fn read_error(e: io::Error) -> PlatformError {
    PlatformError::new(-1, format!("{DAMAGED}: {e}"))
}

fn zip_error(e: zip::result::ZipError, had_password: bool) -> PlatformError {
    use zip::result::ZipError;
    match e {
        ZipError::UnsupportedArchive(m) if m == ZipError::PASSWORD_REQUIRED => password_error(had_password),
        ZipError::InvalidPassword => password_error(true),
        ZipError::Io(e) => io_error(e),
        e => corrupt(&format!("The ZIP archive cannot be read: {e}")),
    }
}

fn stream(plan: &Plan<'_>, out: &mut Out<'_>, password: Option<&Secret>) -> Result<(), PlatformError> {
    let path = std_path(plan.archive);
    let open = || File::open(&path).map_err(io_error).map(BufReader::new);
    let by_ordinal: HashMap<usize, (usize, &str)> = plan.steps.iter().map(|(i, rel)| (plan.index.items[*i].ordinal, (*i, rel.as_str()))).collect();
    let last = by_ordinal.keys().copied().max().unwrap_or(0);
    let put = |out: &mut Out<'_>, i: usize, rel: &str, reader: &mut dyn Read| {
        let item = &plan.index.items[i];
        if item.dir { out.dir(rel) } else { out.file(item, rel, plan.index.declared, reader) }
    };
    match plan.index.format {
        Format::Zip => {
            let mut zip = zip::ZipArchive::new(open()?).map_err(|e| zip_error(e, false))?;
            let aligned = zip.len() == plan.index.entries;
            let pw = password.map(|p| p.0.as_bytes());
            for (i, rel) in &plan.steps {
                let item = &plan.index.items[*i];
                if item.dir {
                    out.dir(rel)?;
                    continue;
                }
                let k = if aligned { Some(item.ordinal) } else { zip.index_for_name(&item.zip_name) };
                let Some(k) = k else { continue };
                let mut f = zip.by_index_with_options(k, zip::ZipReadOptions::new().password(pw)).map_err(|e| zip_error(e, pw.is_some()))?;
                let encrypted = f.encrypted();
                out.file(item, rel, true, &mut f).map_err(|e| if encrypted && e.message.starts_with(DAMAGED) { password_error(true) } else { e })?;
            }
        }
        Format::SevenZ => {
            let a = plan.index.sevenz.as_ref().ok_or_else(|| corrupt("The 7z archive cannot be read"))?;
            let pw = password.map_or_else(sevenz_rust2::Password::empty, |p| sevenz_rust2::Password::new(&p.0));
            let mut src = open()?;
            let mut blocks: Vec<usize> = by_ordinal.keys().filter_map(|&o| a.stream_map.file_block_index.get(o).copied().flatten()).collect();
            blocks.sort_unstable();
            blocks.dedup();
            for (&o, &(i, rel)) in &by_ordinal {
                if a.stream_map.file_block_index.get(o).copied().flatten().is_none() {
                    put(out, i, rel, &mut io::empty())?;
                }
            }
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
            let mut failure: Option<PlatformError> = None;
            for b in blocks {
                let mut o = a.stream_map.block_first_file_index[b];
                let decoder = sevenz_rust2::BlockDecoder::new(threads, b, a, &pw, &mut src);
                let result = decoder.for_each_entries(&mut |_, reader| {
                    let cur = o;
                    o += 1;
                    if let Some(&(i, rel)) = by_ordinal.get(&cur) {
                        if let Err(e) = put(out, i, rel, reader) {
                            failure = Some(e);
                            return Ok(false);
                        }
                    }
                    io::copy(reader, &mut io::sink()).map_err(|e| sevenz_rust2::Error::Io(e, "skip".into()))?;
                    Ok(cur < last)
                });
                if let Some(e) = failure.take() {
                    return Err(e);
                }
                result.map_err(|e| sevenz_error(e, password.is_some()))?;
            }
        }
        Format::Tar | Format::TarGz => {
            let reader: Box<dyn Read> = if plan.index.format == Format::Tar { Box::new(open()?) } else { Box::new(flate2::read::MultiGzDecoder::new(open()?)) };
            let mut a = tar::Archive::new(reader);
            for (o, e) in a.entries().map_err(io_error)?.enumerate() {
                if o > last {
                    break;
                }
                let mut e = e.map_err(read_error)?;
                if let Some(&(i, rel)) = by_ordinal.get(&o) {
                    put(out, i, rel, &mut e)?;
                }
            }
        }
        Format::Gz => {
            if let Some(&(i, rel)) = by_ordinal.get(&0) {
                put(out, i, rel, &mut flate2::read::MultiGzDecoder::new(open()?))?;
            }
        }
        Format::Rar => rar(plan, out, &by_ordinal, last, password)?,
    }
    Ok(())
}

fn rar(plan: &Plan<'_>, out: &mut Out<'_>, by_ordinal: &HashMap<usize, (usize, &str)>, last: usize, password: Option<&Secret>) -> Result<(), PlatformError> {
    let path = std_path(plan.archive);
    let had = password.is_some();
    let mut cursor = match password {
        Some(p) => unrar_ng::Archive::with_password(&path, p.0.as_bytes()).open_for_processing(),
        None => unrar_ng::Archive::new(&path).open_for_processing(),
    }
    .map_err(|e| rar_error(e, had))?;
    let mut o = 0usize;
    while let Some(header) = cursor.read_header().map_err(|e| rar_error(e, had))? {
        if o > last {
            break;
        }
        cursor = match by_ordinal.get(&o) {
            Some(&(i, rel)) if !plan.index.items[i].dir => {
                let item = &plan.index.items[i];
                match out.prepare(item, rel)? {
                    Some((dest, _)) => {
                        let part = Out::part(&dest);
                        let (next, result) = match header.extract_to(&part) {
                            Ok(next) => (next, File::options().write(true).open(&part).map(|f| stamp(&f, item.modified)).map_err(io_error)),
                            Err(e) => {
                                let _ = fs::remove_file(&part);
                                return Err(rar_error(e, had));
                            }
                        };
                        if result.is_ok() {
                            out.job.update(|p| p.bytes += item.size);
                        }
                        out.commit(&part, &dest, result)?;
                        next
                    }
                    None => header.skip().map_err(|e| rar_error(e, had))?,
                }
            }
            Some(&(_, rel)) => {
                out.dir(rel)?;
                header.skip().map_err(|e| rar_error(e, had))?
            }
            None => header.skip().map_err(|e| rar_error(e, had))?,
        };
        if out.job.cancelled() {
            return Err(cancelled());
        }
        o += 1;
    }
    Ok(())
}

fn no_space(need: u64, free: u64) -> PlatformError {
    let (mut a, mut b) = (String::new(), String::new());
    flit_core::fmt::size(&mut a, need);
    flit_core::fmt::size(&mut b, free);
    PlatformError::new(112, format!("Not enough free space: the archive needs {a}, the drive has {b}"))
}

fn check(plan: &Plan<'_>, env: &Env<'_>) -> Result<(u64, u64), PlatformError> {
    let declared = plan.declared();
    if plan.index.declared && declared > BOMB_FLOOR && declared / plan.index.packed.max(1) > BOMB_RATIO {
        return Err(bomb());
    }
    let target = std_path(&plan.target);
    let free = existing_ancestor(&target).and_then(|a| (env.free_space)(a)).unwrap_or(u64::MAX);
    if plan.index.declared && declared > free {
        return Err(no_space(declared, free));
    }
    Ok((BOMB_FLOOR.max(plan.index.packed.saturating_mul(BOMB_RATIO)), free))
}

type Created = Vec<(String, String)>;

fn run(plan: &Plan<'_>, job: &Job, env: &Env<'_>) -> Result<Created, (Created, PlatformError)> {
    let (cap, free) = check(plan, env).map_err(|e| (Vec::new(), e))?;
    let target = std_path(&plan.target);
    let roots: Vec<String> = {
        let mut seen = HashSet::new();
        plan.steps.iter().map(|(_, rel)| first(rel).to_owned()).filter(|r| seen.insert(fold(r))).collect()
    };
    let fresh: Vec<&String> = roots.iter().filter(|r| !target.join(r).exists()).collect();
    let mut out = Out { job, target: target.clone(), target_fs: plan.target.clone(), archive: plan.archive, renamed: HashMap::new(), cap, free, written: 0, skipped: Vec::new(), made: HashSet::new() };
    let result = fs::create_dir_all(&target).map_err(io_error).and_then(|()| stream(plan, &mut out, archive::password(plan.archive).as_ref()));
    let mut created: Vec<(String, String)> = fresh.iter().filter(|r| target.join(r).exists()).map(|r| ((*r).clone(), (*r).clone())).collect();
    created.extend(out.renamed.iter().filter(|(_, new)| target.join(new).exists()).map(|(old, new)| (old.clone(), new.clone())));
    match result {
        Err(e) => Err((created, e)),
        Ok(()) if !out.skipped.is_empty() => Err((created, PlatformError::new(-1, format!("Skipped {} items because folders with the same names exist: {}", out.skipped.len(), out.skipped.join(", "))))),
        Ok(()) => Ok(created),
    }
}

fn named(a: &FsPath, e: PlatformError) -> PlatformError {
    if e.code == archive::PASSWORD_CODE {
        return e;
    }
    PlatformError::new(e.code, format!("{}: {}", flit_core::name::display(a.file_name()), e.message))
}

fn roots_of(index: &Index) -> usize {
    index.items.iter().map(|i| fold(first(&i.path))).collect::<HashSet<_>>().len()
}

pub fn archives(list: &[FsPath], dest: Option<&FsPath>, place: Place, job: &Job, env: &Env<'_>) -> Outcome {
    let mut work = Vec::new();
    for a in list {
        let index = index::open(a, env.utc_offset).map_err(|e| (Vec::new(), named(a, e)))?;
        let base = dest.cloned().or_else(|| a.parent()).ok_or_else(|| (Vec::new(), corrupt("No destination folder")))?;
        let own = match place {
            Place::Here => false,
            Place::OwnFolder => true,
            Place::Smart => roots_of(&index) > 1,
        };
        let target = if own { base.join(archive::stem(a.file_name())) } else { base };
        work.push((a, index, target, own));
    }
    job.update(|p| {
        p.stage = Stage::Running;
        p.total_items = work.iter().map(|(_, i, ..)| i.items.len() as u64).sum();
        p.total_bytes = work.iter().map(|(_, i, ..)| i.items.iter().map(|x| x.size).sum::<u64>()).sum();
    });
    let mut done = Vec::new();
    for (a, index, target, own) in &work {
        let fresh = *own && !std_path(target).exists();
        let plan = Plan { archive: a, index, steps: index.items.iter().enumerate().map(|(i, it)| (i, it.path.clone())).collect(), target: target.clone() };
        let result = run(&plan, job, env);
        let created = match &result {
            Ok(c) | Err((c, _)) => c.clone(),
        };
        if fresh && std_path(target).exists() {
            done.push(((*a).clone(), target.clone()));
        } else {
            done.extend(created.into_iter().map(|(_, new)| ((*a).clone(), target.join(new.as_bytes()))));
        }
        if let Err((_, e)) = result {
            return Err((done, named(a, e)));
        }
        if !index.rejected.is_empty() {
            let names = index.rejected.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
            return Err((done, named(a, PlatformError::new(-1, format!("Skipped {} entries with unsafe names: {names}", index.rejected.len())))));
        }
    }
    Ok(done)
}

pub fn entries(archive: &FsPath, items: &[FsPath], dest: &FsPath, job: &Job, env: &Env<'_>) -> Outcome {
    let index = index::open(archive, env.utc_offset).map_err(|e| (Vec::new(), named(archive, e)))?;
    let mut steps = Vec::new();
    let mut sources: HashMap<String, FsPath> = HashMap::new();
    for item in items {
        let inner = match crate::locate(item) {
            Some((a, inner)) if a == *archive && !inner.is_empty() => inner,
            _ => return Err((Vec::new(), PlatformError::new(-1, "Items from different archives cannot be copied together"))),
        };
        let key = fold(&inner);
        let cut = inner.rfind('/').map_or(0, |k| k + 1);
        let before = steps.len();
        for (i, it) in index.items.iter().enumerate() {
            if it.key == key || (it.key.len() > key.len() && it.key.starts_with(&key) && it.key.as_bytes()[key.len()] == b'/') {
                steps.push((i, it.path[cut..].to_owned()));
            }
        }
        if steps.len() == before {
            return Err((Vec::new(), PlatformError::new(2, format!("{inner} is not in the archive"))));
        }
        sources.insert(inner[cut..].to_owned(), item.clone());
    }
    steps.sort_by_key(|&(i, _)| index.items[i].ordinal);
    job.update(|p| {
        p.stage = Stage::Running;
        p.total_items = steps.len() as u64;
        p.total_bytes = steps.iter().map(|&(i, _)| index.items[i].size).sum();
    });
    let plan = Plan { archive, index: &index, steps, target: dest.clone() };
    let pairs = |c: Vec<(String, String)>| c.into_iter().map(|(old, new)| (sources.get(&old).cloned().unwrap_or_else(|| archive.clone()), dest.join(new.as_bytes()))).collect::<Pairs>();
    match run(&plan, job, env) {
        Ok(c) => Ok(pairs(c)),
        Err((c, e)) => Err((pairs(c), e)),
    }
}
