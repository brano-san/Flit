use crate::fs::{cstring, dents_ino, host, node_at, open_dir, stat_path, Node};
use crate::mounts::{self, Mount};
use crate::trash::Trash;
use flit_core::path::PathKind;
use flit_core::tags::FileKey;
use flit_core::FsPath;
use flit_platform::Resolved;
use std::collections::HashMap;
use std::ffi::{CStr, OsStr};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BY_UUID: &str = "/dev/disk/by-uuid";
const BTRFS: u32 = 0x9123_683E;
const LOCAL: [u32; 11] = [0xEF53, 0x5846_5342, BTRFS, 0xF2F5_2010, 0x3153_464A, 0x5265_4973, 0xCA45_1A4E, 0x2FC1_2FC1, 0x3434, 0x7366_746E, 0xF15F];
const FRESH: Duration = Duration::from_secs(30);

static VOLUMES: Mutex<Vec<(u64, u32, Instant)>> = Mutex::new(Vec::new());

#[repr(C)]
struct Handle {
    bytes: u32,
    _kind: i32,
    id: [u8; 128],
}

fn fnv(seed: u32, bytes: &[u8]) -> u32 {
    bytes.iter().fold(seed, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

pub(crate) fn key_id(ino: u64, btime: i64, handle: &[u8]) -> u64 {
    if btime == 0 && handle.is_empty() {
        return ino;
    }
    ino ^ ((fnv(fnv(0x811C_9DC5, &btime.to_le_bytes()), handle) as u64) << 32)
}

fn handle_at(dir: libc::c_int, name: &CStr) -> Option<Handle> {
    let mut h = Handle { bytes: 128, _kind: 0, id: [0; 128] };
    let mut mount: libc::c_int = 0;
    let r = unsafe { libc::syscall(libc::SYS_name_to_handle_at, dir, name.as_ptr(), &mut h as *mut Handle, &mut mount as *mut libc::c_int, 0) };
    (r == 0 && h.bytes as usize <= h.id.len()).then_some(h)
}

fn id_at(dir: libc::c_int, name: &CStr, node: &Node) -> (u64, bool) {
    let h = handle_at(dir, name);
    let handle = h.as_ref().map_or(&[][..], |h| &h.id[..h.bytes as usize]);
    (key_id(node.ino, node.btime, handle), node.btime != 0 || !handle.is_empty())
}

pub(crate) fn entry_id(dir: libc::c_int, name: &CStr, node: &Node) -> u64 {
    id_at(dir, name, node).0
}

fn statfs(probe: &CStr) -> Option<(u32, [libc::c_int; 2])> {
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(probe.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let fsid: [libc::c_int; 2] = unsafe { std::mem::transmute(st.f_fsid) };
    Some((st.f_type as u32, fsid))
}

fn device(m: &Mount) -> u64 {
    (m.dev.0 as u64) << 32 | m.dev.1 as u64
}

fn identity(dev: u64, probe: &CStr) -> Option<String> {
    let (kind, fsid) = statfs(probe).filter(|f| LOCAL.contains(&f.0))?;
    let by_fsid = (fsid != [0, 0]).then(|| format!("fsid-{:08x}{:08x}", fsid[0] as u32, fsid[1] as u32));
    if kind == BTRFS {
        return by_fsid;
    }
    let mounts = mounts::read().unwrap_or_default();
    let uuid = mounts.iter().find(|m| device(m) == dev).and_then(|m| mounts::label_of(m, &mounts::labels(Path::new(BY_UUID))));
    Some(uuid.map(|u| format!("uuid-{u}")).or(by_fsid).unwrap_or_else(|| format!("dev-{dev:x}")))
}

fn volume_of(dev: u64, probe: &CStr) -> Option<u32> {
    let mut cache = VOLUMES.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|v| v.2.elapsed() < FRESH);
    if let Some(v) = cache.iter().find(|v| v.0 == dev) {
        return Some(v.1);
    }
    let volume = fnv(0x811C_9DC5, identity(dev, probe)?.as_bytes());
    cache.push((dev, volume, Instant::now()));
    Some(volume)
}

fn probe(path: &Path) -> Option<std::ffi::CString> {
    cstring(path.as_os_str().as_bytes()).ok()
}

pub(crate) fn dir_volume(fd: libc::c_int, path: &Path) -> Option<(u64, u32)> {
    let dev = node_at(fd, c".", false).ok()?.dev;
    Some((dev, volume_of(dev, &probe(path)?)?))
}

fn key_of(node: &Node, path: &Path) -> Option<FileKey> {
    let on = if node.is_link() { path.parent()? } else { path };
    Some(FileKey { volume: volume_of(node.dev, &probe(on)?)?, id: entry_id(libc::AT_FDCWD, &probe(path)?, node) })
}

fn node(path: &Path, follow: bool) -> Option<Node> {
    node_at(libc::AT_FDCWD, &probe(path)?, follow).ok()
}

pub fn file_key(path: &FsPath) -> Option<FileKey> {
    let path = host(path)?;
    key_of(&node(path, false)?, path)
}

fn found(path: &Path) -> Option<Resolved> {
    let p = FsPath::parse_exact(path.as_os_str().as_bytes()).filter(|p| p.kind() == PathKind::Unix)?;
    Some(Resolved::Found(p, stat_path(path).ok()?))
}

fn at(key: FileKey, path: &Path) -> Option<Resolved> {
    let n = node(path, false)?;
    (key_of(&n, path)? == key).then(|| found(path)).flatten()
}

fn scan(dir: &Path, keys: &[FileKey], out: &mut [Option<Resolved>]) {
    let Ok(fd) = open_dir(dir) else { return };
    let Some((dev, volume)) = dir_volume(fd.as_raw_fd(), dir) else { return };
    let wanted: Vec<usize> = (0..keys.len()).filter(|&i| out[i].is_none() && keys[i].volume == volume).collect();
    if wanted.is_empty() {
        return;
    }
    let _ = dents_ino(fd.as_raw_fd(), |name, _, ino| {
        for &i in &wanted {
            if out[i].is_some() || keys[i].id as u32 != ino as u32 {
                continue;
            }
            let Ok(n) = node_at(fd.as_raw_fd(), name, false) else { continue };
            if n.dev == dev && id_at(fd.as_raw_fd(), name, &n) == (keys[i].id, true) {
                out[i] = found(&dir.join(OsStr::from_bytes(name.to_bytes())));
            }
        }
        true
    });
}

fn volumes(mounts: &[Mount]) -> HashMap<u32, Vec<PathBuf>> {
    let mut by: HashMap<u32, Vec<PathBuf>> = HashMap::new();
    for m in mounts.iter().filter(|m| m.is_storage()) {
        let Some(c) = probe(m.path()) else { continue };
        if let Some(v) = node_at(libc::AT_FDCWD, &c, true).ok().and_then(|n| volume_of(n.dev, &c)) {
            by.entry(v).or_default().push(m.path().to_path_buf());
        }
    }
    by
}

fn missing(key: FileKey, hint: &Path, mounts: &[Mount], elsewhere: &mut Option<HashMap<u32, Vec<PathBuf>>>) -> Resolved {
    let here = hint.ancestors().skip(1).find_map(|a| Some((a, node(a, true)?)));
    if here.and_then(|(a, n)| volume_of(n.dev, &probe(a)?)) == Some(key.volume) {
        return Resolved::Gone;
    }
    let parts: Vec<&OsStr> = hint.components().filter_map(|c| if let Component::Normal(s) = c { Some(s) } else { None }).collect();
    for point in elsewhere.get_or_insert_with(|| volumes(mounts)).get(&key.volume).into_iter().flatten() {
        if let Some(r) = (0..parts.len()).find_map(|k| at(key, &parts[k..].iter().fold(point.clone(), |p, s| p.join(s)))) {
            return r;
        }
    }
    Resolved::Unavailable
}

pub(crate) fn resolve_with(files: &[(FileKey, &FsPath)], places: impl FnOnce() -> (Vec<Mount>, Vec<PathBuf>)) -> Vec<Resolved> {
    let keys: Vec<FileKey> = files.iter().map(|f| f.0).collect();
    let hints: Vec<Option<&Path>> = files.iter().map(|f| host(f.1)).collect();
    let mut out: Vec<Option<Resolved>> = keys.iter().zip(&hints).map(|(k, h)| at(*k, (*h)?)).collect();
    if out.iter().all(Option::is_some) {
        return out.into_iter().flatten().collect();
    }
    let (mounts, trash) = places();
    let mut dirs: Vec<&Path> = hints.iter().zip(&out).filter(|(_, r)| r.is_none()).filter_map(|(h, _)| h.and_then(Path::parent)).chain(trash.iter().map(PathBuf::as_path)).collect();
    dirs.sort_unstable();
    dirs.dedup();
    for dir in dirs {
        scan(dir, &keys, &mut out);
    }
    let mut elsewhere = None;
    out.into_iter().zip(keys.iter().zip(&hints)).map(|(r, (k, h))| r.unwrap_or_else(|| h.map_or(Resolved::Unavailable, |h| missing(*k, h, &mounts, &mut elsewhere)))).collect()
}

pub fn resolve_files(files: &[(FileKey, &FsPath)]) -> Vec<Resolved> {
    resolve_with(files, || {
        let mounts = mounts::read().unwrap_or_default();
        let tops: Vec<PathBuf> = mounts.iter().filter(|m| m.is_storage()).map(|m| m.path().to_path_buf()).collect();
        let trash = Trash::from_env().map(|t| t.dirs(&tops)).unwrap_or_default().into_iter().map(|d| d.join("files")).collect();
        (mounts, trash)
    })
}

#[cfg(test)]
#[path = "../test/tags.rs"]
mod tests;
