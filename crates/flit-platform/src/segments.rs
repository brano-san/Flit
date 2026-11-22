use crate::{FileOps, Job, OpOutcome, PlatformError, std_path};
use crc32fast::Hasher;
use flit_core::i18n::S;
use flit_core::provider::Stage;
use flit_core::FsPath;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

const BUFFER: usize = 256 * 1024;
const MAX_PARTS: u64 = 999;
const MAX_MANIFEST: u64 = 16 * 1024;

#[path = "segment_manifest.rs"]
mod segment_manifest;

use segment_manifest::{decode_manifest, valid_name, Manifest};

fn err(kind: io::ErrorKind, message: S) -> io::Error {
    io::Error::new(kind, message.en())
}

fn check_cancel(job: &Job) -> io::Result<()> {
    if job.cancelled() { Err(io::Error::new(io::ErrorKind::Interrupted, crate::local::CANCELLED)) } else { Ok(()) }
}

fn open_regular(path: &Path) -> io::Result<File> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(err(io::ErrorKind::InvalidInput, S::SegmentsInvalidSource));
    }
    File::open(path)
}

fn for_set(dir: &FsPath, stem: &str, job: &Job, mut visit: impl FnMut(&str, &str) -> io::Result<()>) -> io::Result<()> {
    let folded = (!dir.case_sensitive()).then(|| stem.to_lowercase());
    for entry in fs::read_dir(std_path(dir))? {
        check_cancel(job)?;
        let entry = entry?;
        let file = entry.file_name();
        let Some(file) = file.to_str() else { continue };
        let Some((base, suffix)) = file.rsplit_once('.') else { continue };
        if folded.as_ref().map_or(stem == base, |lower| *lower == base.to_lowercase()) {
            visit(file, suffix)?;
        }
    }
    Ok(())
}

fn part_number(suffix: &str) -> Option<u16> {
    (suffix.len() == 3 && suffix.bytes().all(|x| x.is_ascii_digit())).then(|| suffix.parse::<u16>().ok()).flatten().filter(|&n| n != 0)
}

pub(crate) struct Staging(pub(crate) FsPath, bool);

impl Staging {
    pub(crate) fn new(dest: &FsPath) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..32 {
            let leaf = format!(".flit-parts-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
            let folder = dest.join(leaf.as_bytes());
            match fs::create_dir(std_path(&folder)) {
                Ok(()) => return Ok(Self(folder, false)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(err(io::ErrorKind::AlreadyExists, S::SegmentsCollision))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.1 {
            let _ = fs::remove_dir_all(std_path(&self.0));
        }
    }
}

impl Staging {
    pub(crate) fn keep(&mut self) {
        self.1 = true;
    }
}

struct Prepared {
    staging: Staging,
    names: Vec<String>,
    unverified: bool,
}

fn begin(job: &Job, bytes: u64, items: u64) {
    job.update(|p| {
        p.stage = Stage::Running;
        p.total_bytes = bytes;
        p.total_items = items;
    });
}

fn stream(mut from: impl Read, mut to: impl Write, buffer: &mut [u8], mut crc: Option<&mut Hasher>, job: &Job) -> io::Result<u64> {
    let mut bytes = 0u64;
    loop {
        check_cancel(job)?;
        let count = from.read(buffer)?;
        if count == 0 { return Ok(bytes); }
        let chunk = &buffer[..count];
        to.write_all(chunk)?;
        if let Some(crc) = &mut crc { crc.update(chunk); }
        bytes += count as u64;
        job.update(|p| p.bytes += count as u64);
    }
}

fn split(source: &FsPath, dest: &FsPath, size: u64, job: &Job) -> io::Result<Prepared> {
    let original = std::str::from_utf8(source.file_name()).ok().filter(|n| valid_name(n))
        .ok_or_else(|| err(io::ErrorKind::InvalidInput, S::SegmentsInvalidSource))?;
    let stem = Path::new(original).file_stem().and_then(|s| s.to_str()).unwrap_or(original);
    let mut from = open_regular(&std_path(source))?;
    let meta = from.metadata()?;
    let len = meta.len();
    if size == 0 || len.div_ceil(size) > MAX_PARTS {
        return Err(err(io::ErrorKind::InvalidInput, S::SegmentsInvalidSize));
    }
    for_set(dest, stem, job, |_, suffix| {
        if part_number(suffix).is_some() || suffix.eq_ignore_ascii_case("crc") {
            return Err(err(io::ErrorKind::AlreadyExists, S::SegmentsCollision));
        }
        Ok(())
    })?;
    let staging = Staging::new(dest)?;
    let mut buffer = vec![0; BUFFER];
    let mut crc = Hasher::new();
    let mut names = Vec::new();
    let count = len.div_ceil(size).max(1);
    begin(job, len, count + 1);
    for k in 1..=count {
        let name = format!("{stem}.{k:03}");
        let out = File::create_new(std_path(&staging.0.join(name.as_bytes())))?;
        let want = (len - (k - 1) * size).min(size);
        if stream((&mut from).take(want), out, &mut buffer, Some(&mut crc), job)? != want {
            return Err(err(io::ErrorKind::InvalidData, S::SegmentsSourceChanged));
        }
        names.push(name);
        job.update(|p| p.items += 1);
    }
    let after = from.metadata()?;
    if len != after.len() || meta.modified()? != after.modified()? {
        return Err(err(io::ErrorKind::InvalidData, S::SegmentsSourceChanged));
    }
    check_cancel(job)?;
    let name = format!("{stem}.crc");
    let manifest = Manifest { file: original.to_owned(), len, checksum: crc.finalize() };
    File::create_new(std_path(&staging.0.join(name.as_bytes())))?.write_all(manifest.render().as_bytes())?;
    names.push(name);
    job.update(|p| p.items += 1);
    Ok(Prepared { staging, names, unverified: false })
}

fn join(sh: &impl FileOps, source: &FsPath, dest: &FsPath, job: &Job) -> io::Result<Prepared> {
    let invalid = || err(io::ErrorKind::InvalidData, S::SegmentsInvalidSet);
    let selected = std::str::from_utf8(source.file_name()).map_err(|_| invalid())?;
    let stem = flit_core::segments::set_stem(selected).ok_or_else(invalid)?;
    let dir = source.parent().ok_or_else(invalid)?;
    let mut parts = Vec::new();
    let mut crc = None;
    for_set(&dir, stem, job, |file, suffix| {
        if suffix.eq_ignore_ascii_case("crc") {
            if crc.replace(dir.join(file.as_bytes())).is_some() { return Err(invalid()); }
        } else if let Some(n) = part_number(suffix) {
            parts.push((n, dir.join(file.as_bytes())));
        }
        Ok(())
    })?;
    let info = if let Some(crc) = crc {
        let mut raw = Vec::new();
        let file = open_regular(&std_path(&crc))?;
        if file.take(MAX_MANIFEST + 1).read_to_end(&mut raw)? as u64 > MAX_MANIFEST {
            return Err(invalid());
        }
        Some(Manifest::parse(&decode_manifest(&raw, |bytes| sh.decode_local_ansi(bytes))).ok_or_else(invalid)?)
    } else {
        None
    };
    parts.sort_unstable_by_key(|(n, _)| *n);
    if parts.is_empty() || parts.iter().enumerate().any(|(i, (n, _))| usize::from(*n) != i + 1) {
        return Err(invalid());
    }
    let name = info.as_ref().map_or(stem, |m| m.file.as_str());
    if !valid_name(name) { return Err(invalid()); }
    if fs::symlink_metadata(std_path(&dest.join(name.as_bytes()))).is_ok() {
        return Err(err(io::ErrorKind::AlreadyExists, S::SegmentsCollision));
    }
    let mut buffer = vec![0; BUFFER];
    let size = if let Some(info) = &info {
        info.len.saturating_mul(2)
    } else {
        parts.iter().try_fold(0u64, |total, (_, part)| {
            Ok::<_, io::Error>(total.saturating_add(open_regular(&std_path(part))?.metadata()?.len()))
        })?
    };
    begin(job, size, parts.len() as u64 * if info.is_some() { 2 } else { 1 });
    let assemble = |out: &mut dyn Write, buffer: &mut [u8]| -> io::Result<()> {
        let mut checksum = Hasher::new();
        let mut total = 0;
        for (_, part) in &parts {
            let file = open_regular(&std_path(part))?;
            let limit = info.as_ref().map_or(u64::MAX, |m| (m.len - total).saturating_add(1));
            total += stream(file.take(limit), &mut *out, buffer, info.as_ref().map(|_| &mut checksum), job)?;
            if info.as_ref().is_some_and(|m| total > m.len) { return Err(invalid()); }
            job.update(|p| p.items += 1);
        }
        if let Some(info) = &info {
            if total != info.len || checksum.finalize() != info.checksum { return Err(invalid()); }
        }
        Ok(())
    };
    if info.is_some() {
        assemble(&mut io::sink(), &mut buffer)?;
    }
    check_cancel(job)?;
    let staging = Staging::new(dest)?;
    let mut output = File::create_new(std_path(&staging.0.join(name.as_bytes())))?;
    assemble(&mut output, &mut buffer)?;
    output.flush()?;
    Ok(Prepared { staging, names: vec![name.to_owned()], unverified: info.is_none() })
}

pub(crate) fn run(sh: &mut impl FileOps, source: &FsPath, dest: &FsPath, part_size: Option<u64>, job: &Job) -> OpOutcome {
    let fail = |e: io::Error| if job.cancelled() { PlatformError::cancelled(-1, crate::local::CANCELLED) } else { PlatformError::from_io(&e, e.to_string()) };
    let work = match part_size {
        Some(size) => split(source, dest, size, job),
        None => join(sh, source, dest, job),
    };
    let prepared = match work.and_then(|p| { check_cancel(job)?; Ok(p) }) {
        Ok(p) => p,
        Err(e) => return OpOutcome { items: Vec::new(), error: Some(fail(e)) },
    };
    let mut outcome = OpOutcome::default();
    let mut published = Vec::new();
    for name in prepared.names {
        let from = prepared.staging.0.join(name.as_bytes());
        let to = dest.join(name.as_bytes());
        if let Err(e) = check_cancel(job) {
            outcome.error = Some(fail(e));
            break;
        }
        let identity = sh.file_identity(&from);
        if let Err(e) = sh.rename_local_noreplace(&from, &to) {
            outcome.error = Some(e);
            break;
        }
        published.push((from, to, identity));
    }
    if outcome.error.is_some() {
        published.retain(|(from, to, identity)| {
            identity.is_none_or(|expected| sh.file_identity(to) == Some(expected))
                && sh.rename_local_noreplace(to, from).is_err()
        });
    } else if prepared.unverified {
        job.notice(S::SegmentsNoCrc);
    }
    outcome.items = published.into_iter().map(|(_, to, _)| (source.clone(), to)).collect();
    outcome
}

#[cfg(test)]
#[path = "../test/segments.rs"]
mod tests;
