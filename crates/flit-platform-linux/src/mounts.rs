use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub const MOUNTINFO: &str = "/proc/self/mountinfo";
pub const BY_LABEL: &str = "/dev/disk/by-label";

const PSEUDO: [&str; 29] = [
    "proc", "sysfs", "tmpfs", "devtmpfs", "devpts", "cgroup", "cgroup2", "mqueue", "debugfs", "tracefs", "securityfs", "pstore", "bpf", "fusectl", "configfs", "hugetlbfs", "autofs", "binfmt_misc", "nsfs", "ramfs", "efivarfs", "rpc_pipefs", "selinuxfs", "rootfs", "squashfs", "fuse.portal", "fuse.gvfsd-fuse", "fuse.snapfuse", "nfsd",
];
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

    pub fn is_storage(&self) -> bool {
        !self.is_pseudo() && !self.is_network() && !self.point.starts_with(b"/snap/")
    }

    pub fn removable(&self) -> bool {
        self.point.starts_with(b"/media/") || self.point.starts_with(b"/run/media/")
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

fn parse_line(line: &[u8]) -> Option<Mount> {
    let fields: Vec<&[u8]> = line.split(|&b| b == b' ').filter(|f| !f.is_empty()).collect();
    let dash = fields.iter().position(|f| *f == b"-")?;
    if dash < 6 || fields.len() < dash + 3 {
        return None;
    }
    let number = |f: &[u8]| std::str::from_utf8(f).ok()?.parse::<u32>().ok();
    let (major, minor) = fields[2].split_at(fields[2].iter().position(|&b| b == b':')?);
    Some(Mount {
        id: number(fields[0])?,
        dev: (number(major)?, number(&minor[1..])?),
        root: unescape(fields[3]),
        point: unescape(fields[4]),
        fstype: String::from_utf8_lossy(&unescape(fields[dash + 1])).into_owned(),
        source: unescape(fields[dash + 2]),
    })
}

pub fn parse(text: &[u8]) -> Vec<Mount> {
    text.split(|&b| b == b'\n').filter_map(parse_line).collect()
}

pub fn read() -> io::Result<Vec<Mount>> {
    Ok(parse(&std::fs::read(MOUNTINFO)?))
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

pub fn visible(mounts: &[Mount], user: &[u8]) -> Vec<Mount> {
    let mut top: Vec<&Mount> = Vec::new();
    for m in mounts {
        top.retain(|t| t.point != m.point);
        top.push(m);
    }
    let mut candidates: Vec<&Mount> = top.into_iter().filter(|m| m.is_storage() && shown_point(&m.point, user)).collect();
    candidates.sort_by_key(|m| m.root.len());
    let mut kept: Vec<&Mount> = Vec::new();
    for m in candidates {
        if !kept.iter().any(|k| k.dev == m.dev && within(&m.root, &k.root)) {
            kept.push(m);
        }
    }
    let rank = |m: &Mount| match m.point.as_slice() {
        b"/" => 0,
        b"/home" => 1,
        _ => 2,
    };
    kept.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.point.cmp(&b.point)));
    kept.into_iter().cloned().collect()
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
    if !m.source.starts_with(b"/") {
        return None;
    }
    let dev = std::fs::canonicalize(OsStr::from_bytes(&m.source)).ok()?;
    labels.get(&dev).cloned()
}

pub fn space(point: &Path) -> Option<(u64, u64)> {
    let c = CString::new(point.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let unit = st.f_frsize as u64;
    Some((st.f_blocks as u64 * unit, st.f_bavail as u64 * unit))
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
