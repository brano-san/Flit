use crate::extract::{cancelled, meta_of, numbered, numbered_by, ticks, Outcome};
use crate::index;
use crate::names::fold;
use crate::{corrupt, io_error, std_path, Env};
use flit_core::archive::{self, Compression, Format};
use flit_core::listing::Meta;
use flit_core::path::FsPath;
use flit_core::provider::{Conflict, Resolve, Stage};
use flit_platform::{Job, PlatformError, CANCELLED};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const LZMA_DICT: [u32; 10] = [256 << 10, 1 << 20, 2 << 20, 4 << 20, 4 << 20, 8 << 20, 8 << 20, 16 << 20, 32 << 20, 64 << 20];

pub(crate) struct Source {
    pub(crate) path: PathBuf,
    pub(crate) from: FsPath,
    pub(crate) name: String,
    pub(crate) dir: bool,
    pub(crate) size: u64,
    pub(crate) modified: i64,
}

fn collect(items: &[FsPath], prefix: &str) -> Result<Vec<Source>, PlatformError> {
    let mut out = Vec::new();
    for item in items {
        let name = flit_core::name::display(item.file_name()).into_owned();
        walk(std_path(item), item.clone(), format!("{prefix}{name}"), &mut out)?;
    }
    Ok(out)
}

fn walk(path: PathBuf, from: FsPath, name: String, out: &mut Vec<Source>) -> Result<(), PlatformError> {
    let link = fs::symlink_metadata(&path).map_err(io_error)?;
    let meta = if link.file_type().is_symlink() { fs::metadata(&path).unwrap_or(link) } else { link };
    let modified = meta.modified().map_or(0, ticks);
    if !meta.is_dir() {
        out.push(Source { path, from, name, dir: false, size: meta.len(), modified });
        return Ok(());
    }
    out.push(Source { path: path.clone(), from: from.clone(), name: name.clone(), dir: true, size: 0, modified });
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(());
    }
    let mut children: Vec<_> = match fs::read_dir(&path) {
        Ok(rd) => rd.filter_map(Result::ok).map(|e| e.file_name()).collect(),
        Err(_) => return Ok(()),
    };
    children.sort();
    for c in children {
        let text = c.to_string_lossy().into_owned();
        walk(path.join(&c), from.join(text.as_bytes()), format!("{name}/{text}"), out)?;
    }
    Ok(())
}

struct Progress<'a, R> {
    inner: R,
    job: &'a Job,
}

impl<R: Read> Read for Progress<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.job.cancelled() {
            return Err(io::Error::other(CANCELLED));
        }
        let n = self.inner.read(buf)?;
        self.job.update(|p| p.bytes += n as u64);
        Ok(n)
    }
}

struct Lazy<'a> {
    path: PathBuf,
    file: Option<File>,
    job: &'a Job,
}

impl Read for Lazy<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.job.cancelled() {
            return Err(io::Error::other(CANCELLED));
        }
        let f = match &mut self.file {
            Some(f) => f,
            None => self.file.insert(File::open(&self.path)?),
        };
        let n = f.read(buf)?;
        if n == 0 {
            self.file = None;
            self.job.update(|p| p.items += 1);
        }
        self.job.update(|p| p.bytes += n as u64);
        Ok(n)
    }
}

pub(crate) fn unreadable(s: &Source, job: &Job) -> Result<(), PlatformError> {
    let meta = Meta { size: s.size, modified: s.modified, created: 0, attrs: 0 };
    match job.resolve(archive::unreadable(&s.from, meta)) {
        Some(_) => {
            job.update(|p| {
                p.items += 1;
                p.bytes += s.size;
            });
            Ok(())
        }
        None => Err(cancelled()),
    }
}

pub(crate) fn write_error(e: io::Error) -> PlatformError {
    if e.to_string() == CANCELLED { cancelled() } else { io_error(e) }
}

struct Split {
    base: PathBuf,
    size: u64,
    parts: Vec<File>,
    pos: u64,
    len: u64,
}

impl Split {
    fn name(base: &Path, k: usize) -> PathBuf {
        let mut s = base.as_os_str().to_owned();
        s.push(format!(".{:03}", k + 1));
        PathBuf::from(s)
    }
}

impl Write for Split {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let k = (self.pos / self.size) as usize;
        while self.parts.len() <= k {
            self.parts.push(File::options().read(true).write(true).create(true).truncate(true).open(Self::name(&self.base, self.parts.len()))?);
        }
        let off = self.pos % self.size;
        let n = buf.len().min((self.size - off) as usize);
        let f = &mut self.parts[k];
        f.seek(SeekFrom::Start(off))?;
        f.write_all(&buf[..n])?;
        self.pos += n as u64;
        self.len = self.len.max(self.pos);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.parts.iter_mut().try_for_each(|f| f.flush())
    }
}

impl Seek for Split {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.pos = match to {
            SeekFrom::Start(p) => p,
            SeekFrom::End(d) => self.len.saturating_add_signed(d),
            SeekFrom::Current(d) => self.pos.saturating_add_signed(d),
        };
        Ok(self.pos)
    }
}

enum Sink {
    One(File),
    Split(Split),
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Sink::One(f) => f.write(buf),
            Sink::Split(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Sink::One(f) => f.flush(),
            Sink::Split(s) => s.flush(),
        }
    }
}

impl Seek for Sink {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match self {
            Sink::One(f) => f.seek(to),
            Sink::Split(s) => s.seek(to),
        }
    }
}

pub(crate) fn zip_time(ticks: i64, offset: i32) -> zip::DateTime {
    let secs = ticks / 10_000_000 - 11_644_473_600 + offset as i64 * 60;
    let (y, m, d) = flit_core::fmt::civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    if y < 1980 {
        return zip::DateTime::default();
    }
    zip::DateTime::from_date_and_time(y.min(2107) as u16, m as u8, d as u8, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8).unwrap_or_default()
}

fn zip_entries<W: Write + Seek>(z: &mut zip::ZipWriter<W>, sources: &[Source], level: u8, password: Option<&str>, job: &Job, env: &Env<'_>) -> Result<(), PlatformError> {
    let method = if level == 0 { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated };
    let zerr = |e: zip::result::ZipError| match e {
        zip::result::ZipError::Io(e) => write_error(e),
        e => corrupt(&e.to_string()),
    };
    for s in sources {
        let base = zip::write::SimpleFileOptions::default().compression_method(method).compression_level((level > 0).then_some(level as i64)).last_modified_time(zip_time(s.modified, env.utc_offset)).large_file(s.size >= 0xFFFF_FFFF);
        let opts = match password {
            Some(pw) if !s.dir => base.with_aes_encryption(zip::AesMode::Aes256, pw),
            _ => base,
        };
        if s.dir {
            z.add_directory(&s.name, opts).map_err(zerr)?;
            job.update(|p| p.items += 1);
            continue;
        }
        let file = match File::open(&s.path) {
            Ok(f) => f,
            Err(_) => {
                unreadable(s, job)?;
                continue;
            }
        };
        z.start_file(&s.name, opts).map_err(zerr)?;
        io::copy(&mut Progress { inner: file, job }, z).map_err(write_error)?;
        job.update(|p| p.items += 1);
    }
    Ok(())
}

fn write_zip<W: Write + Seek>(w: W, sources: &[Source], options: &Compression, job: &Job, env: &Env<'_>) -> Result<W, PlatformError> {
    let Some(password) = &options.password else { return crate::pzip::write(w, sources, options.level, threads(options) as usize, job, env) };
    let mut z = zip::ZipWriter::new(w);
    zip_entries(&mut z, sources, options.level, Some(password.0.as_str()), job, env)?;
    z.finish().map_err(|e| corrupt(&e.to_string()))
}

fn threads(options: &Compression) -> u32 {
    match options.threads {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
        t => t,
    }
}

fn write_7z<W: Write + Seek>(w: W, sources: Vec<Source>, options: &Compression, job: &Job) -> Result<W, PlatformError> {
    use sevenz_rust2::{encoder_options::Lzma2Options, ArchiveEntry, ArchiveWriter, Password, SourceReader};
    let err = |e: sevenz_rust2::Error| match e {
        sevenz_rust2::Error::Io(e, _) => write_error(e),
        e if e.to_string().contains(CANCELLED) => cancelled(),
        e => corrupt(&e.to_string()),
    };
    let mut kept = Vec::with_capacity(sources.len());
    for s in sources {
        if s.dir || File::open(&s.path).is_ok() {
            kept.push(s);
        } else {
            unreadable(&s, job)?;
        }
    }
    let mut z = ArchiveWriter::new(w).map_err(err)?;
    let level = options.level.min(9) as u32;
    let dict = if options.dictionary > 0 { options.dictionary } else { LZMA_DICT[level as usize] };
    let threads = threads(options);
    let mut lzma = if threads > 1 { Lzma2Options::from_level_mt(level, threads, dict as u64) } else { Lzma2Options::from_level(level) };
    lzma.set_dictionary_size(dict);
    if options.word > 0 {
        lzma.set_nice_len(options.word);
    }
    match &options.password {
        Some(pw) => {
            z.set_content_methods(vec![sevenz_rust2::encoder_options::AesEncoderOptions::new(Password::new(&pw.0)).into(), lzma.into()]);
            z.set_encrypt_header(options.encrypt_names);
        }
        None => {
            z.set_content_methods(vec![lzma.into()]);
            z.set_encrypt_header(false);
        }
    }
    let entry = |s: &Source| {
        let mut e = ArchiveEntry::from_path(&s.path, s.name.clone());
        e.is_directory = s.dir;
        e.is_anti_item = s.dir || s.size == 0;
        e
    };
    let (files, rest): (Vec<&Source>, Vec<&Source>) = kept.iter().partition(|s| !s.dir && s.size > 0);
    for s in rest {
        z.push_archive_entry::<&[u8]>(entry(s), None).map_err(err)?;
        job.update(|p| p.items += 1);
    }
    if !files.is_empty() {
        let entries = files.iter().map(|s| entry(s)).collect();
        let readers = files.iter().map(|s| SourceReader::new(Lazy { path: s.path.clone(), file: None, job })).collect();
        z.push_archive_entries(entries, readers).map_err(err)?;
    }
    z.finish().map_err(write_error)
}

fn tar_header(s: &Source) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(if s.dir { tar::EntryType::Directory } else { tar::EntryType::Regular });
    h.set_size(if s.dir { 0 } else { s.size });
    h.set_mode(if s.dir { 0o755 } else { 0o644 });
    h.set_mtime((s.modified / 10_000_000 - 11_644_473_600).max(0) as u64);
    h
}

fn write_tar<W: Write>(w: W, sources: &[Source], job: &Job) -> Result<W, PlatformError> {
    let mut t = tar::Builder::new(w);
    for s in sources {
        let mut h = tar_header(s);
        if s.dir {
            t.append_data(&mut h, &s.name, io::empty()).map_err(write_error)?;
        } else {
            let Ok(file) = File::open(&s.path) else {
                unreadable(s, job)?;
                continue;
            };
            let size = file.metadata().map_or(s.size, |m| m.len());
            h.set_size(size);
            t.append_data(&mut h, &s.name, Progress { inner: file.take(size), job }).map_err(write_error)?;
        }
        job.update(|p| p.items += 1);
    }
    t.into_inner().map_err(write_error)
}

fn write_gz<W: Write>(w: W, s: &Source, level: u8, threads: usize, job: &Job) -> Result<W, PlatformError> {
    let file = File::open(&s.path).map_err(io_error)?;
    let name = s.name.rsplit('/').next().unwrap_or(&s.name).as_bytes().to_vec();
    let mtime = (s.modified / 10_000_000 - 11_644_473_600).clamp(0, u32::MAX as i64) as u32;
    let mut gz = crate::pzip::Gzip::new(w, level, threads, Some(&name), mtime).map_err(write_error)?;
    io::copy(&mut Progress { inner: file, job }, &mut gz).map_err(write_error)?;
    job.update(|p| p.items += 1);
    gz.finish().map_err(write_error)
}

fn write_all(sink: Sink, sources: Vec<Source>, format: Format, options: &Compression, job: &Job, env: &Env<'_>) -> Result<Sink, PlatformError> {
    let flush = |b: BufWriter<Sink>| b.into_inner().map_err(|e| io_error(e.into_error()));
    match format {
        Format::Zip => flush(write_zip(BufWriter::new(sink), &sources, options, job, env)?),
        Format::SevenZ => flush(write_7z(BufWriter::new(sink), sources, options, job)?),
        Format::Tar => flush(write_tar(BufWriter::new(sink), &sources, job)?),
        Format::Gz => flush(write_gz(BufWriter::new(sink), &sources[0], options.level, threads(options) as usize, job)?),
        Format::TarGz => {
            let gz = crate::pzip::Gzip::new(BufWriter::new(sink), options.level, threads(options) as usize, None, 0).map_err(write_error)?;
            flush(write_tar(gz, &sources, job)?.finish().map_err(write_error)?)
        }
        Format::Rar | Format::Lzh => Err(corrupt("RAR and LZH archives cannot be created")),
    }
}

pub fn effective(format: Format, items: &[FsPath]) -> Format {
    let single_file = items.len() == 1 && std_path(&items[0]).is_file();
    if format == Format::Gz && !single_file { Format::TarGz } else { format }
}

pub fn compress(items: &[FsPath], archive: &FsPath, options: &Compression, job: &Job, env: &Env<'_>) -> Outcome {
    let fail = |e| (Vec::new(), e);
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let sources = collect(items, "").map_err(fail)?;
    let format = effective(options.format, items);
    job.update(|p| {
        p.stage = Stage::Running;
        p.total_items = sources.len() as u64;
        p.total_bytes = sources.iter().map(|s| s.size).sum();
    });
    let split = options.volume > 0;
    let mut target = archive.clone();
    let first_name = |t: &FsPath| if split { Split::name(&std_path(t), 0) } else { std_path(t) };
    if let Some(existing) = meta_of(&first_name(&target)) {
        let from = Meta { size: job.progress().total_bytes, ..Meta::default() };
        match job.resolve(Conflict { source: items[0].clone(), target: target.clone(), from, to: existing }) {
            Some(Resolve::Replace) => {}
            Some(Resolve::KeepBoth) => {
                let dir = std_path(&target.parent().unwrap_or_else(|| target.clone()));
                let name = numbered(&dir, &flit_core::name::display(target.file_name()), false);
                target = target.parent().unwrap_or_else(|| target.clone()).join(name.as_bytes());
            }
            Some(Resolve::Skip) => return Ok(Vec::new()),
            None => return Err(fail(cancelled())),
        }
    }
    let path = std_path(&target);
    if split {
        let s = Split { base: path.clone(), size: options.volume.max(64 << 10), parts: Vec::new(), pos: 0, len: 0 };
        let result = write_all(Sink::Split(s), sources, format, options, job, env);
        let made = match &result {
            Ok(Sink::Split(s)) => s.parts.len(),
            _ => (0..).take_while(|&k| Split::name(&path, k).exists()).count(),
        };
        let volume = |k: usize| {
            let mut b = target.as_bytes().to_vec();
            b.extend_from_slice(format!(".{:03}", k + 1).as_bytes());
            FsPath::parse_exact(&b).unwrap_or_else(|| target.clone())
        };
        return match result {
            Ok(_) => Ok((0..made).map(|k| (items[0].clone(), volume(k))).collect()),
            Err(e) => {
                (0..made).for_each(|k| drop(fs::remove_file(Split::name(&path, k))));
                Err(fail(e))
            }
        };
    }
    let part = path.with_file_name(format!("~{}-{}.flit-part", std::process::id(), crate::extract::next_part()));
    let file = File::create_new(&part).map_err(|e| fail(io_error(e)))?;
    let result = write_all(Sink::One(file), sources, format, options, job, env).and_then(|sink| {
        drop(sink);
        fs::rename(&part, &path).map_err(io_error)
    });
    match result {
        Ok(()) => Ok(vec![(items[0].clone(), target)]),
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(fail(e))
        }
    }
}

pub fn add(items: &[FsPath], archive: &FsPath, inner: &str, job: &Job, env: &Env<'_>) -> Outcome {
    let fail = |e| (Vec::new(), e);
    let index = index::open(archive, env.utc_offset).map_err(fail)?;
    if !index.format.can_add() {
        return Err(fail(PlatformError::new(-1, flit_core::i18n::S::ErrorsArchiveNoAdd.en())));
    }
    if items.iter().any(|i| crate::locate(i).is_some_and(|(a, n)| a == *archive || !n.is_empty())) {
        return Err(fail(PlatformError::new(-1, "Items inside archives cannot be added to an archive; extract them first")));
    }
    let prefix = if inner.is_empty() { String::new() } else { format!("{inner}/") };
    let mut sources = collect(items, &prefix).map_err(fail)?;
    let key_prefix = fold(&prefix);
    let mut drop_roots: HashSet<String> = HashSet::new();
    let mut rename: HashMap<String, String> = HashMap::new();
    let mut skip: HashSet<String> = HashSet::new();
    let taken = |k: &str| index.items.iter().any(|i| i.key == k || (i.key.starts_with(k) && i.key.as_bytes().get(k.len()) == Some(&b'/')));
    for item in items {
        let name = flit_core::name::display(item.file_name()).into_owned();
        let key = format!("{key_prefix}{}", fold(&name));
        if !taken(&key) {
            continue;
        }
        let existing = Meta { attrs: 0, ..Meta::default() };
        let from = meta_of(&std_path(item)).unwrap_or_default();
        match job.resolve(Conflict { source: item.clone(), target: archive.join(format!("{prefix}{name}").as_bytes()), from, to: existing }) {
            Some(Resolve::Replace) => {
                drop_roots.insert(key);
            }
            Some(Resolve::KeepBoth) => {
                let fresh = numbered_by(&name, from.attrs & flit_core::listing::attr::DIR != 0, |n| taken(&format!("{key_prefix}{}", fold(n))));
                rename.insert(name, fresh);
            }
            Some(Resolve::Skip) => {
                skip.insert(name);
            }
            None => return Err(fail(cancelled())),
        }
    }
    sources.retain(|s| !skip.contains(s.name[prefix.len()..].split('/').next().unwrap_or("")));
    for s in &mut sources {
        let rest = &s.name[prefix.len()..];
        let root = rest.split('/').next().unwrap_or(rest);
        if let Some(new) = rename.get(root) {
            s.name = format!("{prefix}{new}{}", &rest[root.len()..]);
        }
    }
    job.update(|p| {
        p.stage = Stage::Running;
        p.total_items = sources.len() as u64;
        p.total_bytes = sources.iter().map(|s| s.size).sum();
    });
    let dropped = |key: &str| drop_roots.iter().any(|r| key == r || (key.starts_with(r.as_str()) && key.as_bytes().get(r.len()) == Some(&b'/')));
    let path = std_path(archive);
    let part = path.with_file_name(format!("~{}-{}.flit-part", std::process::id(), crate::extract::next_part()));
    let result = (|| -> Result<(), PlatformError> {
        let mut src = zip::ZipArchive::new(io::BufReader::new(File::open(&path).map_err(io_error)?)).map_err(|e| corrupt(&e.to_string()))?;
        let aligned = src.len() == index.entries;
        let by_ordinal: HashMap<usize, &index::Item> = index.items.iter().map(|i| (i.ordinal, i)).collect();
        let mut z = zip::ZipWriter::new(BufWriter::new(File::create_new(&part).map_err(io_error)?));
        for k in 0..src.len() {
            let item = if aligned { by_ordinal.get(&k).copied() } else { None };
            if item.is_some_and(|i| dropped(&i.key)) {
                continue;
            }
            let f = src.by_index_raw(k).map_err(|e| corrupt(&e.to_string()))?;
            let copied = match item {
                Some(i) => z.raw_copy_file_rename(f, if i.dir { format!("{}/", i.path) } else { i.path.clone() }),
                None => z.raw_copy_file(f),
            };
            copied.map_err(|e| corrupt(&e.to_string()))?;
        }
        zip_entries(&mut z, &sources, 6, None, job, env)?;
        z.finish().map_err(|e| corrupt(&e.to_string()))?.into_inner().map_err(|e| io_error(e.into_error()))?;
        Ok(())
    })()
    .and_then(|()| fs::rename(&part, &path).map_err(io_error));
    crate::forget(archive);
    match result {
        Ok(()) => Ok(Vec::new()),
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(fail(e))
        }
    }
}
