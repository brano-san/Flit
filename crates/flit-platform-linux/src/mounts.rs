use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MOUNTINFO: &str = "/proc/self/mountinfo";
pub const BY_LABEL: &str = "/dev/disk/by-label";

const PSEUDO: [&str; 29] = [
    "proc",
    "sysfs",
    "tmpfs",
    "devtmpfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "mqueue",
    "debugfs",
    "tracefs",
    "securityfs",
    "pstore",
    "bpf",
    "fusectl",
    "configfs",
    "hugetlbfs",
    "autofs",
    "binfmt_misc",
    "nsfs",
    "ramfs",
    "efivarfs",
    "rpc_pipefs",
    "selinuxfs",
    "rootfs",
    "squashfs",
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "fuse.snapfuse",
    "nfsd",
];
const SYSTEM_DIRS: [&[u8]; 18] = [b"/boot", b"/efi", b"/proc", b"/sys", b"/dev", b"/run", b"/tmp", b"/var", b"/usr", b"/etc", b"/root", b"/snap", b"/srv", b"/opt", b"/nix", b"/gnu", b"/mnt/wsl", b"/mnt/wslg"];
const NETWORK: [&str; 16] = ["nfs", "nfs4", "cifs", "smb3", "smbfs", "ncpfs", "afs", "ceph", "glusterfs", "fuse.glusterfs", "fuse.sshfs", "sshfs", "fuse.rclone", "davfs", "fuse.davfs2", "fuse.s3fs"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub id: u32,
    pub dev: (u32, u32),
    pub root: Vec<u8>,
    pub point: Vec<u8>,
    pub fstype: String,
    pub source: Vec<u8>,
}

impl Mount {
    pub fn path(&self) -> &Path {
        Path::new(OsStr::from_bytes(&self.point))
    }

    pub fn is_pseudo(&self) -> bool {
        PSEUDO.contains(&self.fstype.as_str())
    }

    pub fn is_network(&self) -> bool {
        NETWORK.contains(&self.fstype.as_str())
    }

    pub fn remote_access(&self) -> bool {
        remote_access(&self.fstype)
    }

    pub fn is_storage(&self) -> bool {
        !self.is_pseudo() && !self.is_network() && !self.point.starts_with(b"/snap/")
    }

    pub fn is_drive(&self) -> bool {
        self.is_storage() || self.is_network()
    }

    pub fn removable(&self) -> bool {
        !self.is_network() && (self.point.starts_with(b"/media/") || self.point.starts_with(b"/run/media/"))
    }
}

pub fn unescape(field: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(field.len());
    let mut i = 0;
    while i < field.len() {
        let octal = field.get(i + 1..i + 4).filter(|d| field[i] == b'\\' && d.iter().all(|c| (b'0'..=b'7').contains(c)));
        match octal {
            Some(d) => {
                out.push(d.iter().fold(0u8, |v, c| v.wrapping_mul(8) | (c - b'0')));
                i += 4;
            }
            None => {
                out.push(field[i]);
                i += 1;
            }
        }
    }
    out
}

struct MountFields<'a> {
    id: u32,
    dev: (u32, u32),
    root: &'a [u8],
    point: &'a [u8],
    fstype: &'a [u8],
    source: &'a [u8],
}

fn mount_fields(line: &[u8]) -> Option<MountFields<'_>> {
    let mut fields = line.split(|&b| b == b' ').filter(|f| !f.is_empty());
    let mut head = [&[][..]; 6];
    for field in &mut head {
        *field = fields.next()?;
    }
    if head.contains(&b"-".as_slice()) {
        return None;
    }
    fields.find(|&field| field == b"-")?;
    let fstype = fields.next()?;
    let source = fields.next()?;
    let number = |f: &[u8]| std::str::from_utf8(f).ok()?.parse::<u32>().ok();
    let (major, minor) = head[2].split_at(head[2].iter().position(|&b| b == b':')?);
    Some(MountFields { id: number(head[0])?, dev: (number(major)?, number(&minor[1..])?), root: head[3], point: head[4], fstype, source })
}

fn parse_line(line: &[u8]) -> Option<Mount> {
    let fields = mount_fields(line)?;
    Some(Mount { id: fields.id, dev: fields.dev, root: unescape(fields.root), point: unescape(fields.point), fstype: String::from_utf8_lossy(&unescape(fields.fstype)).into_owned(), source: unescape(fields.source) })
}

pub fn parse(text: &[u8]) -> Vec<Mount> {
    text.split(|&b| b == b'\n').filter_map(parse_line).collect()
}

pub fn read() -> io::Result<Vec<Mount>> {
    Ok(parse(&std::fs::read(MOUNTINFO)?))
}

pub fn enclosing<'a>(path: &Path, mounts: &'a [Mount]) -> Option<&'a Mount> {
    mounts.iter().filter(|m| path.starts_with(m.path())).max_by_key(|m| m.point.len())
}

pub(crate) fn volume_key(path: &Path, mounts: &[Mount]) -> Option<Vec<u8>> {
    let (major, minor) = enclosing(path, mounts)?.dev;
    Some(libc::makedev(major, minor).to_le_bytes().to_vec())
}

pub fn remote_points(mounts: &[Mount]) -> Vec<PathBuf> {
    mounts.iter().filter(|m| m.remote_access()).map(|m| m.path().to_path_buf()).collect()
}

fn remote_access(fstype: &str) -> bool {
    NETWORK.contains(&fstype) || matches!(fstype, "autofs" | "fuse.gvfsd-fuse")
}

fn parse_remote_points(text: &[u8]) -> Vec<PathBuf> {
    text.split(|&b| b == b'\n')
        .filter_map(mount_fields)
        .filter_map(|fields| {
            let fstype = if fields.fstype.contains(&b'\\') { std::borrow::Cow::Owned(unescape(fields.fstype)) } else { std::borrow::Cow::Borrowed(fields.fstype) };
            remote_access(&String::from_utf8_lossy(&fstype)).then(|| PathBuf::from(std::ffi::OsString::from_vec(unescape(fields.point))))
        })
        .collect()
}

pub(crate) fn read_remote_points() -> Vec<PathBuf> {
    std::fs::read(MOUNTINFO).map_or_else(|_| Vec::new(), |text| parse_remote_points(&text))
}

pub(crate) fn remote_children<'a>(dir: &Path, points: &'a [PathBuf]) -> Vec<&'a [u8]> {
    points.iter().filter(|p| p.parent() == Some(dir)).filter_map(|p| p.file_name().map(OsStrExt::as_bytes)).collect()
}

pub fn remote_path(path: &Path, mounts: &[Mount]) -> bool {
    enclosing(path, mounts).is_some_and(Mount::remote_access)
}

pub(crate) fn server_key(path: &Path, mounts: &[Mount]) -> Option<Vec<u8>> {
    let mount = enclosing(path, mounts)?;
    let source = &mount.source;
    let server = if mount.fstype == "fuse.gvfsd-fuse" {
        let folder = path.strip_prefix(mount.path()).ok()?.components().next()?;
        let entry = folder.as_os_str().as_bytes();
        let properties = entry.iter().position(|&c| c == b':')?;
        entry[properties + 1..].split(|&c| c == b',').find_map(|p| p.strip_prefix(b"server=").or_else(|| p.strip_prefix(b"host=")))?
    } else if mount.is_network() {
        if let Some(unc) = source.strip_prefix(b"//") {
            unc.split(|&c| c == b'/').next()?
        } else if let Some(start) = source.windows(3).position(|p| p == b"://") {
            source[start + 3..].split(|&c| c == b'/').next()?.rsplit(|&c| c == b'@').next()?
        } else if let Some(end) = source.windows(2).position(|p| p == b":/").or_else(|| source.ends_with(b":").then(|| source.len() - 1)) {
            source[..end].rsplit(|&c| c == b'@').next()?
        } else {
            source
        }
    } else {
        return None;
    };
    (!server.is_empty()).then(|| server.to_ascii_lowercase())
}

fn within(root: &[u8], of: &[u8]) -> bool {
    of == b"/" || root == of || (root.starts_with(of) && root.get(of.len()) == Some(&b'/'))
}

fn shown_point(point: &[u8], user: &[u8]) -> bool {
    let child_of = |dir: &[u8]| point.strip_prefix(dir).is_some_and(|rest| !rest.is_empty() && !rest.contains(&b'/'));
    let media: Vec<u8> = [b"/media/", user, b"/"].concat();
    let run_media: Vec<u8> = [b"/run/media/", user, b"/"].concat();
    point == b"/" || point == b"/home" || child_of(b"/mnt/") || (!user.is_empty() && (child_of(&media) || child_of(&run_media)))
}

fn on_top(mounts: &[Mount]) -> Vec<&Mount> {
    let mut top: Vec<&Mount> = Vec::new();
    for m in mounts {
        top.retain(|t| t.point != m.point);
        top.push(m);
    }
    top
}

fn rank(m: &Mount) -> u8 {
    match m.point.as_slice() {
        b"/" => 0,
        b"/home" => 1,
        _ => 2,
    }
}

fn in_order(mut kept: Vec<&Mount>) -> Vec<Mount> {
    kept.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.point.cmp(&b.point)));
    kept.into_iter().cloned().collect()
}

pub fn visible(mounts: &[Mount], user: &[u8]) -> Vec<Mount> {
    let mut candidates: Vec<&Mount> = on_top(mounts).into_iter().filter(|m| m.is_storage() && shown_point(&m.point, user)).collect();
    candidates.sort_by_key(|m| m.root.len());
    let mut kept: Vec<&Mount> = Vec::new();
    for m in candidates {
        if !kept.iter().any(|k| k.dev == m.dev && within(&m.root, &k.root)) {
            kept.push(m);
        }
    }
    in_order(kept)
}

fn under(point: &[u8], dir: &[u8]) -> bool {
    point.strip_prefix(dir).is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
}

fn disk_point(point: &[u8], user: &[u8]) -> bool {
    let users_own = |base: &[u8]| !user.is_empty() && point.strip_prefix(base).and_then(|r| r.strip_prefix(user)).is_some_and(|r| r.len() > 1 && r.starts_with(b"/"));
    if users_own(b"/run/media/") || users_own(b"/media/") {
        return true;
    }
    if under(point, b"/media") {
        return point.get(b"/media/".len()..).is_some_and(|r| !r.is_empty() && !r.contains(&b'/'));
    }
    !SYSTEM_DIRS.iter().any(|d| under(point, d))
}

pub fn disks(mounts: &[Mount], user: &[u8]) -> Vec<Mount> {
    let mut candidates: Vec<&Mount> = on_top(mounts).into_iter().filter(|m| m.is_drive() && disk_point(&m.point, user)).collect();
    candidates.sort_by_key(|m| (rank(m), m.root.len(), m.point.len()));
    let mut kept: Vec<&Mount> = Vec::new();
    for m in candidates {
        if !kept.iter().any(|k| k.dev == m.dev) {
            kept.push(m);
        }
    }
    in_order(kept)
}

pub fn user_name() -> Vec<u8> {
    let uid = unsafe { libc::getuid() };
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 4096];
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let r = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut found) };
    if r == 0 && !found.is_null() && !pw.pw_name.is_null() {
        return unsafe { std::ffi::CStr::from_ptr(pw.pw_name) }.to_bytes().to_vec();
    }
    std::env::var_os("USER").map(|u| u.as_bytes().to_vec()).unwrap_or_default()
}

pub fn unescape_label(name: &[u8]) -> String {
    let hex = |b: Option<&u8>| b.and_then(|&b| (b as char).to_digit(16)).map(|d| d as u8);
    let mut out = Vec::with_capacity(name.len());
    let mut i = 0;
    while i < name.len() {
        match (name[i], name.get(i + 1), hex(name.get(i + 2)), hex(name.get(i + 3))) {
            (b'\\', Some(b'x'), Some(h), Some(l)) => {
                out.push(h << 4 | l);
                i += 4;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn labels(dir: &Path) -> HashMap<PathBuf, String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return HashMap::new() };
    entries.filter_map(Result::ok).filter_map(|e| Some((std::fs::canonicalize(e.path()).ok()?, unescape_label(e.file_name().as_bytes())))).collect()
}

pub fn label_of(m: &Mount, labels: &HashMap<PathBuf, String>) -> Option<String> {
    if m.is_network() || !m.source.starts_with(b"/") {
        return None;
    }
    let dev = std::fs::canonicalize(OsStr::from_bytes(&m.source)).ok()?;
    labels.get(&dev).cloned()
}

pub fn space(point: &Path) -> Option<(u64, u64)> {
    let c = CString::new(point.as_os_str().as_bytes()).ok()?;
    let raw = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if raw < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatvfs(fd.as_raw_fd(), &mut st) } != 0 {
        return None;
    }
    let unit = st.f_frsize as u64;
    Some((st.f_blocks as u64 * unit, st.f_bavail as u64 * unit))
}

pub const NETWORK_SPACE_WAIT: Duration = Duration::from_millis(1500);

static PROBING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn probing() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    PROBING.lock().unwrap_or_else(|e| e.into_inner())
}

fn start_probe(point: &Path, probe: fn(&Path) -> Option<(u64, u64)>) -> Option<crossbeam_channel::Receiver<Option<(u64, u64)>>> {
    let mut busy = probing();
    if busy.iter().any(|p| p == point) {
        return None;
    }
    busy.push(point.to_path_buf());
    drop(busy);
    let (tx, rx) = crossbeam_channel::bounded(1);
    let owned = point.to_path_buf();
    let spawned = std::thread::Builder::new().name("flit-statvfs".into()).spawn(move || {
        let found = probe(&owned);
        probing().retain(|p| *p != owned);
        let _ = tx.send(found);
    });
    if spawned.is_err() {
        probing().retain(|p| p != point);
        return None;
    }
    Some(rx)
}

pub fn spaces_within(points: &[PathBuf], wait: Duration, probe: fn(&Path) -> Option<(u64, u64)>) -> Vec<Option<(u64, u64)>> {
    let deadline = Instant::now() + wait;
    let started: Vec<_> = points.iter().map(|p| start_probe(p, probe)).collect();
    started.into_iter().map(|rx| rx?.recv_deadline(deadline).ok().flatten()).collect()
}

pub fn block_size(path: &Path) -> Option<u64> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    [st.f_frsize as u64, st.f_bsize as u64].into_iter().find(|&b| b > 0)
}

pub fn watch(path: &Path, changed: impl Fn() + Send + 'static) -> io::Result<()> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    std::thread::Builder::new().name("flit-mounts".into()).spawn(move || loop {
        let mut pfd = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLPRI, revents: 0 };
        let ready = unsafe { libc::poll(&mut pfd, 1, -1) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if pfd.revents & (libc::POLLPRI | libc::POLLERR) != 0 {
            changed();
        }
        if pfd.revents & libc::POLLNVAL != 0 {
            return;
        }
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "../test/mounts.rs"]
mod tests;
