use crate::fileops::std_path;
use crate::fs::{open_dir, open_relative, NtFs, DRIVE_FIXED};
use crate::instance::user_sid;
use crate::walk_index::Handle;
use flit_core::listing::{attr, Listing, Meta};
use flit_core::recycle::{self as bin, BIN_FOLDER};
use flit_core::sort::CaseMode;
use flit_core::{name, FsError, FsPath, ListingBuilder};
use flit_platform::{Fs, OpOutcome, PlatformError};
use rayon::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::os::windows::fs::MetadataExt;
use windows::core::PCWSTR;
use windows::Wdk::Storage::FileSystem::FILE_NON_DIRECTORY_FILE;
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, MoveFileExW, ReadFile, FILE_ATTRIBUTE_DIRECTORY, FILE_LIST_DIRECTORY, FILE_READ_DATA, MOVE_FILE_FLAGS};

const HEADER: usize = 24;
const V1_PATH_UNITS: usize = 260;
const MAX_INFO: usize = HEADER + 4 + 2 * 32768;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Info {
    pub size: u64,
    pub deleted: i64,
    pub path: Vec<u16>,
}

pub(crate) fn parse_info(b: &[u8]) -> Option<Info> {
    let u64_at = |at: usize| Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?));
    let (version, size, deleted) = (u64_at(0)?, u64_at(8)?, u64_at(16)? as i64);
    let units = match version {
        1 => {
            let rest = b.get(HEADER..)?;
            &rest[..rest.len().min(2 * V1_PATH_UNITS)]
        }
        2 => {
            let n = u32::from_le_bytes(b.get(HEADER..HEADER + 4)?.try_into().ok()?) as usize;
            b.get(HEADER + 4..HEADER + 4 + 2 * n)?
        }
        _ => return None,
    };
    let path: Vec<u16> = units.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    (!path.is_empty()).then_some(Info { size, deleted, path })
}

pub(crate) fn roots() -> Vec<FsPath> {
    let Some(sid) = user_sid() else { return Vec::new() };
    let mask = unsafe { GetLogicalDrives() };
    (b'A'..=b'Z')
        .filter(|&d| mask & (1 << (d - b'A')) != 0)
        .filter(|&d| unsafe { GetDriveTypeW(PCWSTR([d as u16, b':' as u16, b'\\' as u16, 0].as_ptr())) } == DRIVE_FIXED)
        .filter_map(|d| FsPath::parse(&format!(r"{}:\$Recycle.Bin\{sid}", d as char)))
        .collect()
}

pub(crate) fn info_file(recycled: &FsPath) -> Option<FsPath> {
    let id = recycled.file_name().strip_prefix(b"$R")?;
    let dir = recycled.parent()?;
    dir.parent()?.file_name().eq_ignore_ascii_case(BIN_FOLDER).then(|| dir.join(&[&b"$I"[..], id].concat()))
}

fn entries(root: &FsPath) -> Option<Listing> {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut go);
    NtFs.read_dir(root, &mut b).ok()?;
    Some(b.listing)
}

thread_local! {
    static INFO_BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0; MAX_INFO]);
}

fn read_info(dir: &Handle, name: &[u8]) -> Option<Info> {
    let mut wide = Vec::with_capacity(name.len());
    name::to_utf16(name, &mut wide);
    let h = open_relative(dir, &wide, FILE_READ_DATA.0, FILE_NON_DIRECTORY_FILE.0)?;
    INFO_BUFFER.with_borrow_mut(|buf| {
        let mut n = 0u32;
        unsafe { ReadFile(h.0, Some(buf), Some(&mut n), None) }.ok()?;
        parse_info(&buf[..n as usize])
    })
}

pub(crate) fn list(roots: &[FsPath], out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    for root in roots {
        let (Some(l), Ok(dir)) = (entries(root), open_dir(root, FILE_LIST_DIRECTORY.0, false).map(Handle)) else { continue };
        let payloads: HashMap<&[u8], u32> = (0..l.len() as u32).filter_map(|i| Some((l.name(i).strip_prefix(b"$R")?, l.entries[i as usize].attrs))).collect();
        let items: Vec<(&[u8], u32, u32)> = (0..l.len() as u32).filter_map(|i| {
            let id = l.name(i).strip_prefix(b"$I")?;
            Some((id, i, *payloads.get(id)?))
        }).collect();
        let read = |&(_, i, _): &(&[u8], u32, u32)| read_info(&dir, l.name(i));
        let infos: Vec<Option<Info>> = match NtFs.seek_penalty(root) {
            Some(true) => items.iter().map(read).collect(),
            _ => items.par_iter().map(read).collect(),
        };
        for ((id, _, attrs), info) in items.into_iter().zip(infos) {
            let Some(info) = info else { continue };
            let name = info.path.rsplit(|&c| c == b'\\' as u16).next().unwrap_or_default();
            let Some(origin) = FsPath::from_utf16(&info.path).and_then(|p| p.parent()) else { continue };
            let recycled = root.join(&[&b"$R"[..], id].concat());
            let meta = Meta { size: info.size, modified: info.deleted, created: info.deleted, attrs: attrs & !(attr::HIDDEN | attr::SYSTEM) | attr::SIZE_KNOWN };
            bin::push(out, name, meta, &recycled, &origin);
        }
        if !out.batch_done() {
            break;
        }
    }
    Ok(())
}

fn remove(p: &FsPath) -> std::io::Result<()> {
    let path = std_path(p);
    let meta = std::fs::symlink_metadata(&path)?;
    if meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
        return std::fs::remove_dir_all(&path);
    }
    std::fs::remove_file(&path)
}

fn failure(p: &FsPath, e: std::io::Error) -> PlatformError {
    PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("{}: {e}", p.display()))
}

pub(crate) fn purge(pairs: &[(FsPath, FsPath)]) -> OpOutcome {
    let mut out = OpOutcome::default();
    for (recycled, original) in pairs {
        let Some(info) = info_file(recycled) else {
            out.error = Some(PlatformError::new(-1, flit_core::tr_en!(flit_core::i18n::S::ErrorsEngineNotInBin, path = recycled.display())));
            continue;
        };
        match remove(recycled) {
            Ok(()) => {
                let _ = remove(&info);
                out.items.push((recycled.clone(), original.clone()));
            }
            Err(e) => out.error = Some(failure(original, e)),
        }
    }
    out
}

const DIRECT_BYTES: u64 = 1 << 30;
const ID_CHARS: &[u8; 36] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

fn same_drive(root: &FsPath, p: &FsPath) -> bool {
    let (r, p) = (root.as_bytes(), p.as_bytes());
    p.len() > 2 && p[1] == b':' && r.len() > 2 && r[..2].eq_ignore_ascii_case(&p[..2])
}

fn now_filetime() -> i64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (since.as_nanos() / 100) as i64 + 116_444_736_000_000_000
}

fn info_record(size: u64, deleted: i64, path: &[u16]) -> Vec<u8> {
    let mut b: Vec<u8> = [2u64.to_le_bytes(), size.to_le_bytes(), deleted.to_le_bytes()].concat();
    b.extend((path.len() as u32).to_le_bytes());
    b.extend(path.iter().flat_map(|c| c.to_le_bytes()));
    b
}

fn next_id(seed: &mut u64, ext: &[u8]) -> Vec<u8> {
    let mut id: Vec<u8> = (0..6)
        .map(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            ID_CHARS[(*seed % ID_CHARS.len() as u64) as usize]
        })
        .collect();
    id.extend_from_slice(ext);
    id
}

pub(crate) fn recycle_into(root: &FsPath, p: &FsPath, budget: &mut u64, seed: &mut u64) -> Option<FsPath> {
    let meta = std::fs::symlink_metadata(std_path(p)).ok().filter(|m| m.file_attributes() & FILE_ATTRIBUTE_DIRECTORY.0 == 0)?;
    *budget = budget.checked_sub(meta.len())?;
    let name = p.file_name();
    let ext = name.iter().rposition(|&c| c == b'.').map_or(&b""[..], |i| &name[i..]);
    let record = info_record(meta.len(), now_filetime(), &p.to_wide(false));
    for _ in 0..8 {
        let id = next_id(seed, ext);
        let (info, payload) = (root.join(&[&b"$I"[..], &id].concat()), root.join(&[&b"$R"[..], &id].concat()));
        let mut f = match std::fs::File::create_new(std_path(&info)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        };
        let (src, dst) = (p.to_wide(true), payload.to_wide(true));
        let moved = f.write_all(&record).is_ok() && unsafe { MoveFileExW(PCWSTR(src.as_ptr()), PCWSTR(dst.as_ptr()), MOVE_FILE_FLAGS(0)) }.is_ok();
        drop(f);
        if moved {
            return Some(payload);
        }
        let _ = std::fs::remove_file(std_path(&info));
        return None;
    }
    None
}

pub(crate) fn trash(paths: &[FsPath]) -> (Vec<(FsPath, FsPath)>, Vec<FsPath>) {
    let roots = roots();
    let (mut done, mut left, mut budget, mut seed) = (Vec::new(), Vec::new(), DIRECT_BYTES, now_filetime() as u64 | 1);
    for p in paths {
        match roots.iter().find(|r| same_drive(r, p)).and_then(|r| recycle_into(r, p, &mut budget, &mut seed)) {
            Some(payload) => done.push((p.clone(), payload)),
            None => left.push(p.clone()),
        }
    }
    (done, left)
}

pub(crate) fn count(roots: &[FsPath]) -> usize {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut go);
    let _ = list(roots, &mut b);
    b.listing.len()
}

pub(crate) fn empty(roots: &[FsPath]) -> OpOutcome {
    let mut out = OpOutcome::default();
    for root in roots {
        let Some(l) = entries(root) else { continue };
        let names = (0..l.len() as u32).map(|i| l.name(i));
        let (payloads, infos): (Vec<&[u8]>, Vec<&[u8]>) = names.filter(|n| n.starts_with(b"$R") || n.starts_with(b"$I")).partition(|n| n.starts_with(b"$R"));
        for n in payloads {
            let p = root.join(n);
            match remove(&p) {
                Ok(()) => out.items.push((p.clone(), p)),
                Err(e) => out.error = Some(failure(&p, e)),
            }
        }
        for n in infos {
            if std::fs::symlink_metadata(std_path(&root.join(&[&b"$R"[..], &n[2..]].concat()))).is_err() {
                let _ = remove(&root.join(n));
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "../test/recycle.rs"]
pub(crate) mod tests;
