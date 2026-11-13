use super::{check, top, Engine, Tally};
use crate::fs::{names, node_at, open_dir_at, Node};
use flit_core::provider::{Access, AccessChange, AccessEdit, Enclosed, FileOp, ModeEdit};
use flit_core::FsPath;
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

const PERMS: u32 = 0o7777;
const UNCHANGED: u32 = u32::MAX;

fn access_of(node: &Node) -> Access {
    Access { mode: node.mode, uid: node.uid, gid: node.gid }
}

fn target(node: &Node, perms: u32, uid: u32, gid: u32) -> Access {
    let mode = if node.is_link() { node.mode } else { node.mode & !PERMS | perms & PERMS };
    Access { mode, uid, gid }
}

fn chmod_at(dir: RawFd, name: &CStr, node: &Node, perms: u32) -> io::Result<()> {
    let r = check(unsafe { libc::fchmodat(dir, name.as_ptr(), perms as libc::mode_t, libc::AT_SYMLINK_NOFOLLOW) });
    match r {
        Err(e) if matches!(e.raw_os_error(), Some(libc::EOPNOTSUPP | libc::ENOSYS | libc::EINVAL)) => {
            let now = node_at(dir, name, false)?;
            if now.is_link() || now.ino != node.ino {
                return Err(io::Error::other("the item changed while its permissions were being set"));
            }
            check(unsafe { libc::fchmodat(dir, name.as_ptr(), perms as libc::mode_t, 0) })
        }
        r => r,
    }
}

fn change_at(dir: RawFd, name: &CStr, node: &Node, to: Access) -> io::Result<()> {
    let owned = (to.uid, to.gid) != (node.uid, node.gid);
    if owned {
        let uid = if to.uid == node.uid { UNCHANGED } else { to.uid };
        let gid = if to.gid == node.gid { UNCHANGED } else { to.gid };
        check(unsafe { libc::fchownat(dir, name.as_ptr(), uid, gid, libc::AT_SYMLINK_NOFOLLOW) })?;
    }
    if !node.is_link() && (owned || to.mode & PERMS != node.mode & PERMS) {
        chmod_at(dir, name, node, to.mode & PERMS)?;
    }
    Ok(())
}

fn listed(dir: RawFd, name: &CStr) -> io::Result<(OwnedFd, Vec<CString>)> {
    let fd = open_dir_at(dir, name)?;
    let list = names(fd.as_raw_fd())?;
    Ok((fd, list))
}

fn denied(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EACCES | libc::EPERM))
}

impl Engine<'_> {
    fn stepped(&self) {
        self.job.update(|p| p.items += 1);
    }

    fn performed_access(&self, changes: Vec<AccessChange>) {
        if !changes.is_empty() {
            self.job.performed(FileOp::Access(changes));
        }
    }

    fn apply_access(&mut self, dir: RawFd, name: &CStr, path: &FsPath, node: &Node, to: Access, out: &mut Vec<AccessChange>) {
        self.stepped();
        let from = access_of(node);
        if to == from {
            return;
        }
        match change_at(dir, name, node, to) {
            Ok(()) => {
                out.push(AccessChange { item: path.clone(), from, to });
                self.done.push((path.clone(), path.clone()));
            }
            Err(e) => self.fail(path, &e),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn edit_at(&mut self, dir: RawFd, name: &CStr, path: &FsPath, node: &Node, mode: ModeEdit, edit: &AccessEdit, out: &mut Vec<AccessChange>) {
        let to = target(node, mode.apply(node.mode & PERMS), edit.owner.unwrap_or(node.uid), edit.group.unwrap_or(node.gid));
        let Some(inside) = edit.enclosed.filter(|_| node.is_dir()) else { return self.apply_access(dir, name, path, node, to, out) };
        match listed(dir, name) {
            Ok((fd, list)) => {
                self.edit_children(&fd, &list, path, node, inside, edit, out);
                self.apply_access(dir, name, path, node, to, out);
            }
            Err(e) if denied(&e) => {
                self.apply_access(dir, name, path, node, to, out);
                match listed(dir, name) {
                    Ok((fd, list)) => self.edit_children(&fd, &list, path, node, inside, edit, out),
                    Err(e) => self.fail(path, &e),
                }
            }
            Err(e) => {
                self.fail(path, &e);
                self.apply_access(dir, name, path, node, to, out);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn edit_children(&mut self, fd: &OwnedFd, list: &[CString], path: &FsPath, node: &Node, inside: Enclosed, edit: &AccessEdit, out: &mut Vec<AccessChange>) {
        for n in list {
            if self.stopped() {
                return;
            }
            let p = path.join(n.to_bytes());
            match node_at(fd.as_raw_fd(), n, false) {
                Ok(child) if child.is_dir() && child.mounted_on(node.dev) => self.stepped(),
                Ok(child) => {
                    let mode = if child.is_dir() { inside.folders } else { inside.files };
                    self.edit_at(fd.as_raw_fd(), n, &p, &child, mode, edit, out);
                }
                Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
                Err(e) => self.fail(&p, &e),
            }
        }
    }

    pub(super) fn set_access(&mut self, items: &[FsPath], edit: &AccessEdit) {
        self.preparing();
        let tops = self.tops(items);
        let mut total = Tally::default();
        for t in tops.iter().flatten() {
            match edit.enclosed {
                Some(_) => self.tally(t.dir.as_raw_fd(), &t.name, &t.node, false, &mut total),
                None => total.entries += 1,
            }
        }
        self.running(Tally { entries: total.entries, bytes: 0 });
        let mut out = Vec::new();
        for (item, t) in items.iter().zip(&tops) {
            if self.stopped() {
                break;
            }
            let Some(t) = t else { continue };
            self.edit_at(t.dir.as_raw_fd(), &t.name, item, &t.node, edit.mode, edit, &mut out);
        }
        self.performed_access(out);
    }

    pub(super) fn access(&mut self, changes: &[AccessChange]) {
        self.running(Tally { entries: changes.len() as u64, bytes: 0 });
        let mut out = Vec::new();
        for c in changes {
            if self.stopped() {
                break;
            }
            match top(&c.item) {
                Ok(t) => {
                    let to = target(&t.node, c.to.mode, c.to.uid, c.to.gid);
                    self.apply_access(t.dir.as_raw_fd(), &t.name, &c.item, &t.node, to, &mut out);
                }
                Err(e) if e.raw_os_error() == Some(libc::ENOENT) => self.note(format!("{}: the item no longer exists", c.item.display())),
                Err(e) => self.fail(&c.item, &e),
            }
        }
        self.performed_access(out);
    }
}

#[cfg(test)]
#[path = "../../test/access.rs"]
mod tests;
