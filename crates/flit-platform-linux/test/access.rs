use super::super::run;
use crate::temp::Temp;
use flit_core::provider::{AccessEdit, Enclosed, FileOp, ModeEdit, Resolve};
use flit_core::undo::{inverse, Step};
use flit_core::provider::OpReport;
use flit_core::FsPath;
use flit_platform::Job;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

fn fp(p: &Path) -> FsPath {
    FsPath::parse_exact(p.as_os_str().as_bytes()).unwrap()
}

fn mode(p: &Path) -> u32 {
    std::fs::symlink_metadata(p).unwrap().mode() & 0o7777
}

fn chmod(p: &Path, m: u32) {
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
}

fn performed(op: FileOp) -> (OpReport, Option<String>) {
    let job = Job::detached(Resolve::Skip);
    let out = run(&op, &job, None);
    (OpReport { op: job.take_performed().unwrap_or(op), items: out.items }, out.error.map(|e| e.message))
}

#[test]
fn enclosed_items_take_their_own_masks_and_undo_restores_every_mode() {
    let t = Temp::new("access-tree");
    let top = t.path().join("top");
    let inner = top.join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    let (a, b) = (top.join("a.txt"), inner.join("b.sh"));
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();
    chmod(&a, 0o600);
    chmod(&b, 0o755);
    chmod(&inner, 0o700);
    chmod(&top, 0o700);
    let edit = ModeEdit { set: 0o055, clear: 0 };
    let op = FileOp::SetAccess { items: vec![fp(&top)], edit: AccessEdit { mode: edit, enclosed: Some(Enclosed { files: edit.only(0o666), folders: edit }), ..Default::default() } };
    let (report, error) = performed(op);
    assert_eq!(error, None);
    assert_eq!((mode(&top), mode(&inner), mode(&a), mode(&b)), (0o755, 0o755, 0o644, 0o755), "files never gain execute from a folder change");
    assert_eq!(report.items.len(), 3, "an unchanged file is not reported");
    let Some(Step::Run(undo)) = inverse(&report) else { panic!("not undoable: {report:?}") };
    let (_, error) = performed(undo);
    assert_eq!(error, None);
    assert_eq!((mode(&top), mode(&inner), mode(&a), mode(&b)), (0o700, 0o700, 0o600, 0o755));
}

#[test]
fn removing_access_from_a_folder_still_reaches_what_it_holds() {
    let t = Temp::new("access-shut");
    let dir = t.path().join("d");
    std::fs::create_dir(&dir).unwrap();
    let f = dir.join("f");
    std::fs::write(&f, b"x").unwrap();
    chmod(&f, 0o644);
    let edit = ModeEdit { set: 0, clear: 0o077 };
    let op = FileOp::SetAccess { items: vec![fp(&dir)], edit: AccessEdit { mode: edit, enclosed: Some(Enclosed { files: edit, folders: edit }), ..Default::default() } };
    let (_, error) = performed(op);
    assert_eq!(error, None);
    assert_eq!((mode(&dir), mode(&f)), (0o700, 0o600));
}

#[test]
fn a_group_of_the_user_is_set_and_reported_for_undo() {
    let t = Temp::new("access-group");
    let f = t.path().join("g");
    std::fs::write(&f, b"x").unwrap();
    let before = std::fs::metadata(&f).unwrap().gid();
    let n = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    let mut groups = vec![0 as libc::gid_t; n.max(0) as usize];
    unsafe { libc::getgroups(n, groups.as_mut_ptr()) };
    let Some(&other) = groups.iter().find(|&&g| g != before) else { return };
    let (report, error) = performed(FileOp::SetAccess { items: vec![fp(&f)], edit: AccessEdit { group: Some(other), ..Default::default() } });
    assert_eq!(error, None);
    assert_eq!(std::fs::metadata(&f).unwrap().gid(), other);
    let FileOp::Access(changes) = &report.op else { panic!("{report:?}") };
    assert_eq!((changes[0].from.gid, changes[0].to.gid), (before, other));
    let Some(Step::Run(undo)) = inverse(&report) else { panic!() };
    assert_eq!(performed(undo).1, None);
    assert_eq!(std::fs::metadata(&f).unwrap().gid(), before);
}
