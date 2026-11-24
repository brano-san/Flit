use super::*;
use crate::temp::Temp;
use flit_core::provider::{Failure, OpCtl, Recover};
use std::ffi::OsStr;
use std::os::unix::fs::{symlink, FileExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

fn fp(p: &Path) -> FsPath {
    FsPath::parse_exact(p.as_os_str().as_bytes()).unwrap()
}

fn quiet(policy: Resolve) -> Job {
    Job::detached(policy)
}

fn ok(out: OpOutcome) -> Pairs {
    assert!(out.error.is_none(), "{:?}", out.error);
    out.items
}

fn answering(choice: Resolve, all: bool) -> (Job, Arc<Mutex<Vec<Conflict>>>) {
    let ctl = OpCtl::new(None);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (c, s) = (ctl.clone(), seen.clone());
    let job = Job::new(
        ctl,
        Box::new(|_| {}),
        Box::new(move |conflict| {
            s.lock().unwrap().push(conflict);
            c.answer(choice, all);
        }),
    );
    (job, seen)
}

fn write(p: &Path, text: &str) {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(p, text).unwrap();
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn listing(dir: &Path) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().as_bytes().to_vec()).collect();
    v.sort();
    v
}

#[test]
fn unsupported_rename_noreplace_checks_the_destination_before_using_renameat() {
    let t = Temp::new("ops-rename-fallback");
    let source = t.path().join("source");
    let target = t.path().join("target");
    write(&source, "new");
    write(&target, "keep");
    let dir = open_dir(t.path()).unwrap();
    let dir = dir.as_raw_fd();
    for no_support in [libc::EINVAL, libc::ENOSYS, libc::EOPNOTSUPP] {
        let attempt = || rename_fallback(dir, c"source", dir, c"target", Err(errno(no_support)));
        assert_eq!(attempt().unwrap_err().raw_os_error(), Some(libc::EEXIST));
        assert_eq!(read(&target), "keep");
        assert_eq!(read(&source), "new");
    }
    std::fs::remove_file(&target).unwrap();
    rename_fallback(dir, c"source", dir, c"target", Err(errno(libc::EINVAL))).unwrap();
    assert_eq!(read(&target), "new");
    assert!(!source.exists());
}

#[test]
fn cancelled_delete_restores_names_and_restart_leaves_them_in_place() {
    let t = Temp::new("ops-cancel-staged");
    let state = t.path().join("state");
    let directory = t.path().join("selected");
    std::fs::create_dir(&directory).unwrap();
    let paths: Vec<_> = (0..10)
        .map(|index| {
            let path = directory.join(format!("item-{index}"));
            std::fs::create_dir(&path).unwrap();
            fp(&path)
        })
        .collect();
    let stage = directory.join(".flit-stage-1.flit-partial");
    std::fs::create_dir(&stage).unwrap();
    let ctl = OpCtl::default();
    let cancel = ctl.clone();
    let listed = directory.clone();
    let job = Job::new(
        ctl,
        Box::new(move |progress| {
            if progress.stage == Stage::Running && progress.items == 0 {
                let mut names = Vec::new();
                crate::fs::scan(&listed, |name, _| {
                    names.push(name.to_vec());
                    true
                })
                .unwrap();
                assert!(names.is_empty());
                assert_eq!(std::fs::read_dir(&listed).unwrap().count(), 11);
                std::thread::sleep(Duration::from_millis(40));
            } else if progress.items == 1 {
                cancel.cancel();
            }
        }),
        Box::new(|_| {}),
    );
    let mut engine = Engine::new(&job);
    engine.delete(&paths, Some(&state));
    assert_eq!(engine.done, vec![(paths[0].clone(), paths[0].clone())]);
    assert_eq!(std::fs::read_dir(state.join("pending-deletes")).unwrap().count(), 0);
    assert!(!host(&paths[0]).unwrap().exists());
    recover_pending(&state, |_| false);
    assert!(paths[1..].iter().all(|p| host(p).unwrap().exists()));
    assert!(stage.exists());
}

#[test]
fn a_cancelled_delete_left_under_its_staged_name_is_visible_and_can_be_recovered() {
    let t = Temp::new("ops-recover-cancelled");
    let state = t.path().join("state");
    let directory = t.path().join("data");
    let original = directory.join("item");
    write(&original, "keep this");
    let occupied = original.clone();
    let ctl = OpCtl::default();
    let cancel = ctl.clone();
    let job = Job::new(
        ctl,
        Box::new(move |progress| {
            if progress.stage == Stage::Running {
                write(&occupied, "occupant");
                cancel.cancel();
            }
        }),
        Box::new(|_| {}),
    );
    let mut engine = Engine::new(&job);
    engine.delete(&[fp(&original)], Some(&state));
    let mut staged = None;
    crate::fs::scan(&directory, |name, meta| {
        if flit_platform::pending::is_delete_name(name) {
            assert_eq!(meta.attrs & flit_core::listing::attr::HIDDEN, 0);
            staged = Some(directory.join(OsStr::from_bytes(name)));
        }
        true
    })
    .unwrap();
    let staged = staged.unwrap();
    assert!(engine.done.is_empty() && engine.error().unwrap().message.contains(staged.to_str().unwrap()));
    recover_pending(&state, |_| false);
    assert_eq!(read(&original), "occupant");
    assert_eq!(read(&staged), "keep this");
    std::fs::remove_file(&original).unwrap();
    recover_pending(&state, |_| false);
    assert_eq!(read(&original), "keep this");
    assert!(!staged.exists());
    assert_eq!(std::fs::read_dir(state.join("pending-deletes")).unwrap().count(), 0);
}

#[test]
fn copy_keeps_mode_times_links_and_odd_names() {
    let t = Temp::new("ops-copy");
    let src = t.path().join("src");
    let odd = src.join("a:b\\");
    write(&odd, "hello");
    std::fs::set_permissions(&odd, std::fs::Permissions::from_mode(0o640)).unwrap();
    let past = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    std::fs::File::options().write(true).open(&odd).unwrap().set_modified(past).unwrap();
    symlink("../outside", src.join("link")).unwrap();
    let sub = src.join("sub");
    write(&sub.join(OsStr::from_bytes(b"x\xFF")), "bytes");
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o750)).unwrap();
    let fifo = cstring(src.join("pipe").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let dst = t.path().join("dst");
    std::fs::create_dir(&dst).unwrap();
    let out = run(&FileOp::Copy { items: vec![fp(&src)], dest: fp(&dst) }, &quiet(Resolve::Skip), None);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items, [(fp(&src), fp(&dst.join("src")))]);
    let copy = dst.join("src");
    assert_eq!(read(&copy.join("a:b\\")), "hello");
    let meta = std::fs::metadata(copy.join("a:b\\")).unwrap();
    assert_eq!(meta.mode() & 0o777, 0o640);
    assert_eq!(meta.modified().unwrap(), past);
    assert_eq!(meta.uid(), std::fs::metadata(&odd).unwrap().uid());
    assert_eq!(std::fs::read_link(copy.join("link")).unwrap(), PathBuf::from("../outside"));
    assert_eq!(std::fs::metadata(copy.join("sub")).unwrap().mode() & 0o777, 0o750);
    assert_eq!(read(&copy.join("sub").join(OsStr::from_bytes(b"x\xFF"))), "bytes");
    assert!(std::fs::symlink_metadata(copy.join("pipe")).unwrap().file_type().is_fifo());
    assert_eq!(read(&odd), "hello");
}

#[test]
fn copies_into_the_same_folder_get_a_copy_suffix() {
    let t = Temp::new("ops-dup");
    let file = t.file("a.txt", 3);
    std::fs::create_dir(t.path().join("d")).unwrap();
    let dir = fp(t.path());
    for _ in 0..2 {
        let out = run(&FileOp::Copy { items: vec![fp(&file), fp(&t.path().join("d"))], dest: dir.clone() }, &quiet(Resolve::Skip), None);
        assert!(out.error.is_none(), "{:?}", out.error);
    }
    assert_eq!(listing(t.path()), [&b"a - Copy (2).txt"[..], b"a - Copy.txt", b"a.txt", b"d", b"d - Copy", b"d - Copy (2)"]);
}

fn sparse_file(p: &Path, size: u64, data_at: &[u64]) {
    let f = std::fs::File::create(p).unwrap();
    f.set_len(size).unwrap();
    for &at in data_at {
        f.write_all_at(b"data", at).unwrap();
    }
}

fn copied(t: &Temp, src: &Path) -> PathBuf {
    let dst = t.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    ok(run(&FileOp::Copy { items: vec![fp(src)], dest: fp(&dst) }, &quiet(Resolve::Skip), None));
    dst.join(src.file_name().unwrap())
}

#[test]
fn sparse_files_are_copied_without_filling_their_holes() {
    let t = Temp::new("ops-sparse");
    let src = t.path().join("disk.img");
    sparse_file(&src, 64 << 20, &[0, 32 << 20, (64 << 20) - 4]);
    let copy = copied(&t, &src);
    let meta = std::fs::metadata(&copy).unwrap();
    assert_eq!(meta.len(), 64 << 20);
    assert!(meta.blocks() * 512 < 4 << 20, "the copy takes {} bytes on disk", meta.blocks() * 512);
    assert!(std::fs::read(&copy).unwrap() == std::fs::read(&src).unwrap());
}

#[test]
fn files_that_are_all_hole_end_in_a_hole_or_have_none_copy_byte_for_byte() {
    let t = Temp::new("ops-holes");
    let empty = t.path().join("empty.img");
    sparse_file(&empty, 10 << 20, &[]);
    let tail = t.path().join("tail.img");
    sparse_file(&tail, 8 << 20, &[0]);
    let dense = t.file("dense.bin", 3 << 20);
    for src in [&empty, &tail, &dense] {
        let copy = copied(&t, src);
        assert_eq!(std::fs::metadata(&copy).unwrap().len(), std::fs::metadata(src).unwrap().len(), "{}", src.display());
        assert!(std::fs::read(&copy).unwrap() == std::fs::read(src).unwrap(), "{}", src.display());
    }
    let dense_copy = std::fs::metadata(t.path().join("dst/dense.bin")).unwrap();
    assert!(dense_copy.blocks() * 512 >= 3 << 20);
}

fn set_xattr(p: &Path, name: &str, value: &[u8]) {
    let (p, name) = (cstring(p.as_os_str().as_bytes()).unwrap(), CString::new(name).unwrap());
    assert_eq!(unsafe { libc::setxattr(p.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0) }, 0, "{}", io::Error::last_os_error());
}

fn xattrs(p: &Path) -> Vec<(Vec<u8>, Vec<u8>)> {
    let fd = std::fs::File::open(p).unwrap();
    let names = xattr_names(fd.as_raw_fd());
    let mut out: Vec<_> = names.split(|&b| b == 0).filter(|n| !n.is_empty()).map(|n| (n.to_vec(), xattr_value(fd.as_raw_fd(), &CString::new(n).unwrap()).unwrap())).collect();
    out.sort();
    out
}

fn acl(uid: u32) -> Vec<u8> {
    let entry = |tag: u16, perm: u16, id: u32| [tag.to_le_bytes().as_slice(), &perm.to_le_bytes(), &id.to_le_bytes()].concat();
    [2u32.to_le_bytes().to_vec(), entry(1, 6, u32::MAX), entry(2, 4, uid), entry(4, 4, u32::MAX), entry(0x10, 4, u32::MAX), entry(0x20, 0, u32::MAX)].concat()
}

#[test]
fn copies_keep_extended_attributes_and_access_lists_of_files_and_folders() {
    let t = Temp::new("ops-xattr");
    let src = t.path().join("src");
    let file = src.join("a.txt");
    write(&file, "hello");
    set_xattr(&file, "user.xdg.comment", b"keep me");
    set_xattr(&file, "system.posix_acl_access", &acl(12345));
    set_xattr(&src, "user.xdg.tags", b"red");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    let before = xattrs(&file);
    let copy = copied(&t, &src);
    assert_eq!(xattrs(&copy.join("a.txt")), before);
    assert_eq!(xattrs(&copy), [(b"user.xdg.tags".to_vec(), b"red".to_vec())]);
    assert_eq!(std::fs::metadata(copy.join("a.txt")).unwrap().mode() & 0o777, 0o640);
    assert_eq!(xattrs(&file), before);
}

#[test]
fn plain_files_gain_no_attributes_and_selinux_labels_follow_moves_only() {
    let t = Temp::new("ops-noxattr");
    let file = t.file("plain.txt", 4);
    let copy = copied(&t, &file);
    assert!(xattrs(&copy).iter().all(|(n, _)| !n.starts_with(b"user.")), "{:?}", xattrs(&copy));
    assert!(copied_xattr(b"security.selinux", true));
    assert!(!copied_xattr(b"security.selinux", false));
    assert!(copied_xattr(b"user.xdg.tags", false));
    assert!(copied_xattr(b"system.posix_acl_default", false));
    assert!(!copied_xattr(b"", true));
}

#[test]
fn conflicts_keep_both_replace_skip_and_merge() {
    let t = Temp::new("ops-conflict");
    let (src, dst) = (t.path().join("src"), t.path().join("dst"));
    write(&src.join("f"), "new");
    write(&dst.join("f"), "old");
    let copy = FileOp::Copy { items: vec![fp(&src.join("f"))], dest: fp(&dst) };
    let (job, seen) = answering(Resolve::KeepBoth, false);
    let out = run(&copy, &job, None);
    assert_eq!(out.items, [(fp(&src.join("f")), fp(&dst.join("f (2)")))]);
    let c = seen.lock().unwrap()[0].clone();
    assert_eq!((c.source, c.target, c.from.size, c.to.size), (fp(&src.join("f")), fp(&dst.join("f")), 3, 3));
    assert_eq!(read(&dst.join("f (2)")), "new");
    write(&src.join("f"), "newer");
    let out = run(&copy, &quiet(Resolve::Replace), None);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(read(&dst.join("f")), "newer");
    assert!(!listing(&dst).iter().any(|n| n.ends_with(b".flit-partial")));
    write(&src.join("f"), "ignored");
    run(&copy, &quiet(Resolve::Skip), None);
    assert_eq!(read(&dst.join("f")), "newer");
    write(&src.join("d/a"), "a");
    write(&src.join("d/b"), "b-new");
    write(&dst.join("d/b"), "b-old");
    write(&dst.join("d/c"), "c");
    let (job, seen) = answering(Resolve::Replace, true);
    let out = run(&FileOp::Move { items: vec![fp(&src.join("d"))], dest: fp(&dst) }, &job, None);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(seen.lock().unwrap().len(), 1, "apply to all answers the conflict inside the merged folder");
    assert_eq!(listing(&dst.join("d")), [&b"a"[..], b"b", b"c"]);
    assert_eq!(read(&dst.join("d/b")), "b-new");
    assert!(!src.join("d").exists());
    write(&src.join("k"), "file");
    std::fs::create_dir(dst.join("k")).unwrap();
    let out = run(&FileOp::Copy { items: vec![fp(&src.join("k"))], dest: fp(&dst) }, &quiet(Resolve::Replace), None);
    assert_eq!(out.items, [(fp(&src.join("k")), fp(&dst.join("k (2)")))]);
}

#[test]
fn replace_older_and_skip_identical_decide_each_file_of_a_merged_folder() {
    let t = Temp::new("ops-older");
    let (src, dst) = (t.path().join("src"), t.path().join("dst"));
    let at = |p: &Path, body: &str, hours: u64| {
        write(p, body);
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + hours * 3600)).unwrap();
    };
    at(&src.join("f/newer"), "new", 2);
    at(&dst.join("f/newer"), "old!", 1);
    at(&src.join("f/older"), "new", 1);
    at(&dst.join("f/older"), "old!", 2);
    at(&src.join("f/same"), "same", 1);
    at(&dst.join("f/same"), "same", 1);
    let copy = FileOp::Copy { items: vec![fp(&src.join("f"))], dest: fp(&dst) };
    let (job, seen) = answering(Resolve::ReplaceOlder, true);
    ok(run(&copy, &job, None));
    assert_eq!(seen.lock().unwrap().len(), 1, "only the folder is asked about");
    assert_eq!([read(&dst.join("f/newer")), read(&dst.join("f/older")), read(&dst.join("f/same"))], ["new", "old!", "same"]);
    at(&dst.join("f/newer"), "old!", 1);
    let ctl = OpCtl::new(Some(Resolve::SkipSame));
    let (answer, asked) = (ctl.clone(), Arc::new(Mutex::new(0)));
    let counted = asked.clone();
    let job = Job::new(
        ctl,
        Box::new(|_| {}),
        Box::new(move |_| {
            *counted.lock().unwrap() += 1;
            answer.answer(Resolve::KeepBoth, false);
        }),
    );
    ok(run(&copy, &job, None));
    assert_eq!(*asked.lock().unwrap(), 2);
    assert_eq!(listing(&dst.join("f")), [&b"newer"[..], b"newer (2)", b"older", b"older (2)", b"same"], "the identical file is skipped and the others are asked about");
}

fn asking_room(answer: Option<Resolve>) -> (Job, Arc<Mutex<Vec<Conflict>>>) {
    let ctl = OpCtl::new(Some(Resolve::Replace));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (c, s) = (ctl.clone(), seen.clone());
    let job = Job::asking(
        ctl,
        Box::new(|_| {}),
        Box::new(move |conflict| {
            s.lock().unwrap().push(conflict);
            match answer {
                Some(a) => c.answer(a, false),
                None => c.cancel(),
            }
        }),
        Box::new(|_| {}),
    );
    (job, seen)
}

#[test]
fn copies_and_moves_to_another_device_ask_when_the_free_space_is_short() {
    let t = Temp::new("ops-room");
    write(&t.path().join("src/f"), "0123456789");
    std::fs::create_dir_all(t.path().join("dst")).unwrap();
    let copy = |job: &Job| {
        let mut e = Engine::new(job);
        e.free = |_| Some(4);
        e.copy(&[fp(&t.path().join("src/f"))], &fp(&t.path().join("dst")));
        (e.done, e.first)
    };
    let (job, seen) = asking_room(None);
    let (done, _) = copy(&job);
    assert!(done.is_empty() && job.cancelled() && !t.path().join("dst/f").exists());
    let c = seen.lock().unwrap()[0].clone();
    assert!(limits::is_short_of_space(&c) && (c.from.size, c.to.size) == (10, 4), "the Replace policy does not answer the question");
    let (job, _) = asking_room(Some(limits::ROOM_ANYWAY));
    assert_eq!(copy(&job).0.len(), 1);
    assert_eq!(read(&t.path().join("dst/f")), "0123456789");
    std::fs::remove_file(t.path().join("dst/f")).unwrap();
    let (done, first) = copy(&quiet(Resolve::Replace));
    assert!(done.is_empty() && first.is_some_and(|e| e.contains("not enough free space")), "nobody to ask: refused before it starts");

    let shm = Path::new("/dev/shm");
    let far = std::fs::metadata(shm).is_ok_and(|m| m.dev() != std::fs::metadata(t.path()).unwrap().dev());
    if !far {
        return eprintln!("skipped: /dev/shm is not another device");
    }
    let other = shm.join(format!("flit-test-room-{}", std::process::id()));
    std::fs::create_dir_all(&other).unwrap();
    let pairs = [(fp(&t.path().join("src/f")), fp(&other.join("f")))];
    let (job, seen) = asking_room(None);
    let mut e = Engine::new(&job);
    e.free = |_| Some(4);
    let placed = e.moves(&pairs, false);
    let asked = seen.lock().unwrap().len();
    let left = t.path().join("src/f").exists();
    let _ = std::fs::remove_dir_all(&other);
    assert_eq!((placed, asked, left), (vec![None], 1, true), "a move to another device checks the free space first");
    let job = quiet(Resolve::Skip);
    let mut e = Engine::new(&job);
    e.free = |_| Some(4);
    let near = [(fp(&t.path().join("src/f")), fp(&t.path().join("dst/g")))];
    assert_eq!(e.moves(&near, false), vec![Some(fp(&t.path().join("dst/g")))], "a rename on the same device needs no space");
}

#[test]
fn moves_create_missing_parents_and_cross_devices_by_copy_then_delete() {
    let t = Temp::new("ops-move");
    write(&t.path().join("a/f"), "f");
    write(&t.path().join("a/d/g"), "g");
    let to = t.path().join("new/parent/f2");
    let out = run(&FileOp::MoveTo(vec![(fp(&t.path().join("a/f")), fp(&to))]), &quiet(Resolve::Skip), None);
    assert_eq!(ok(out), [(fp(&t.path().join("a/f")), fp(&to))]);
    assert_eq!(read(&to), "f");
    let job = quiet(Resolve::Skip);
    let mut e = Engine::new(&job);
    let src = fp(&t.path().join("a/d"));
    let top = top(&src).unwrap();
    let dest_dir = open_dir(&t.path().join("new")).unwrap();
    let r = e.transfer(top.dir.as_raw_fd(), &top.name, &src, dest_dir.as_raw_fd(), fp(&t.path().join("new/d")), &top.node, Mode::Across);
    assert!(matches!(r, Res::Whole(ref p) if *p == fp(&t.path().join("new/d"))));
    assert_eq!(read(&t.path().join("new/d/g")), "g");
    assert!(!t.path().join("a/d").exists());
}

#[test]
fn delete_never_follows_links_out_of_the_tree() {
    let t = Temp::new("ops-delete");
    write(&t.path().join("keep/precious"), "p");
    let doomed = t.path().join("doomed");
    write(&doomed.join("a"), "a");
    write(&doomed.join("sub/b"), "b");
    symlink("../keep", doomed.join("link")).unwrap();
    symlink(t.path().join("keep"), doomed.join("sub/abs")).unwrap();
    std::fs::set_permissions(doomed.join("sub"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let out = run(&FileOp::Delete(vec![fp(&doomed)]), &quiet(Resolve::Skip), None);
    std::fs::set_permissions(doomed.join("sub"), std::fs::Permissions::from_mode(0o755)).ok();
    if unsafe { libc::geteuid() } != 0 {
        assert!(out.error.is_some(), "a read-only folder keeps its entries");
        assert_eq!(read(&t.path().join("keep/precious")), "p");
        let out = run(&FileOp::Delete(vec![fp(&doomed)]), &quiet(Resolve::Skip), None);
        assert_eq!(ok(out), [(fp(&doomed), fp(&doomed))]);
    } else {
        assert_eq!(ok(out), [(fp(&doomed), fp(&doomed))]);
    }
    assert!(!doomed.exists());
    symlink(t.path().join("keep"), t.path().join("toplink")).unwrap();
    run(&FileOp::Delete(vec![fp(&t.path().join("toplink"))]), &quiet(Resolve::Skip), None);
    assert!(std::fs::symlink_metadata(t.path().join("toplink")).is_err());
    assert_eq!(read(&t.path().join("keep/precious")), "p");
}

#[test]
fn mount_points_are_recognised_without_touching_them() {
    let proc_root = top(&FsPath::parse("/proc").unwrap()).unwrap();
    assert!(proc_root.node.mounted_on(proc_root.parent_dev));
    let t = Temp::new("ops-mount");
    std::fs::create_dir(t.path().join("plain")).unwrap();
    let plain = top(&fp(&t.path().join("plain"))).unwrap();
    assert!(!plain.node.mounted_on(plain.parent_dev));
}

#[test]
fn rename_and_new_folder_take_linux_names() {
    let t = Temp::new("ops-rename");
    let a = t.file("a", 1);
    t.file("taken", 1);
    let out = run(&FileOp::Rename { item: fp(&a), name: b"taken".to_vec() }, &quiet(Resolve::Skip), None);
    assert!(out.error.unwrap().message.contains("already exists"));
    let out = run(&FileOp::Rename { item: fp(&a), name: b"x\\y:z.".to_vec() }, &quiet(Resolve::Skip), None);
    assert_eq!(out.items, [(fp(&a), fp(&t.path().join("x\\y:z.")))]);
    let out = run(&FileOp::Rename { item: fp(&t.path().join("x\\y:z.")), name: b"no/slash".to_vec() }, &quiet(Resolve::Skip), None);
    assert!(out.error.is_some());
    std::fs::create_dir(t.path().join("New folder")).unwrap();
    let out = run(&FileOp::NewFolder { dir: fp(t.path()), name: b"New folder".to_vec() }, &quiet(Resolve::Skip), None);
    let made = fp(&t.path().join("New folder (2)"));
    assert_eq!(out.items, [(made.clone(), made)]);
    let out = run(&FileOp::Copy { items: vec![FsPath::parse(r"C:\x").unwrap()], dest: fp(t.path()) }, &quiet(Resolve::Skip), None);
    assert_eq!(out.error.unwrap().message, NOT_LINUX);
}

fn umask() -> u32 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find_map(|l| l.strip_prefix("Umask:")).unwrap();
    u32::from_str_radix(line.trim(), 8).unwrap()
}

#[test]
fn new_scripts_are_executable_and_other_new_files_are_not() {
    let t = Temp::new("ops-new-script");
    let dir = fp(t.path());
    let tpl = t.file("tool.sh", 4);
    std::fs::set_permissions(&tpl, std::fs::Permissions::from_mode(0o755)).unwrap();
    let made = |name: &str, template: Template| {
        let out = ok(run(&FileOp::NewFile { dir: dir.clone(), name: name.as_bytes().to_vec(), template }, &quiet(Resolve::Skip), None));
        assert_eq!(out, [(dir.join(name.as_bytes()), dir.join(name.as_bytes()))]);
        std::fs::metadata(t.path().join(name)).unwrap().permissions().mode() & 0o777
    };
    let mask = umask();
    assert_eq!(made("New shell script.sh", Template::Data(b"#!/bin/sh\n\n".to_vec())), 0o777 & !mask);
    assert_eq!(read(&t.path().join("New shell script.sh")), "#!/bin/sh\n\n");
    assert_eq!(made("notes.md", Template::Data(b"# Notes\n".to_vec())), 0o666 & !mask);
    assert_eq!(made("empty", Template::Empty), 0o666 & !mask);
    assert_eq!(made("copy.sh", Template::Copy(fp(&tpl))), 0o666 & !mask);
}

#[test]
fn trash_restores_through_the_agent_with_conflicts() {
    let t = Temp::new("ops-restore");
    let trash = Trash::new(t.path().join("data/Trash"), unsafe { libc::getuid() });
    let v = t.file("v", 2);
    let out = run(&FileOp::Trash(vec![fp(&v)]), &quiet(Resolve::Skip), Some(&trash));
    let trashed = fp(&t.path().join("data/Trash/files/v"));
    assert_eq!(ok(out), [(fp(&v), trashed.clone())]);
    let info = t.path().join("data/Trash/info/v.trashinfo");
    assert!(info.exists());
    let out = run(&FileOp::Restore(vec![(trashed.clone(), fp(&v))]), &quiet(Resolve::Skip), Some(&trash));
    assert_eq!(ok(out), [(trashed.clone(), fp(&v))]);
    assert!(v.exists() && !info.exists());
    run(&FileOp::Trash(vec![fp(&v)]), &quiet(Resolve::Skip), Some(&trash));
    write(&v, "new");
    let (job, seen) = answering(Resolve::KeepBoth, false);
    let out = run(&FileOp::Restore(vec![(trashed.clone(), fp(&v))]), &job, Some(&trash));
    assert_eq!(out.items, [(trashed.clone(), fp(&t.path().join("v (2)")))]);
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(!info.exists());
    assert_eq!(read(&v), "new");
    run(&FileOp::Trash(vec![fp(&v)]), &quiet(Resolve::Skip), Some(&trash));
    let out = run(&FileOp::Restore(vec![(trashed.clone(), fp(&t.path().join("elsewhere")))]), &quiet(Resolve::Skip), Some(&trash));
    assert!(out.error.unwrap().message.contains("came from"), "the Trash entry must match");
    assert!(info.exists());
    let out = run(&FileOp::Restore(vec![(fp(&t.path().join("v (2)")), fp(&v))]), &quiet(Resolve::Skip), Some(&trash));
    assert!(out.error.is_some(), "only items inside a Trash folder are restored");
    assert!(t.path().join("v (2)").exists());
}

#[test]
fn cancelling_removes_the_partial_copy() {
    let t = Temp::new("ops-cancel");
    let big = t.file("big", 1 << 20);
    let dst = t.path().join("dst");
    std::fs::create_dir(&dst).unwrap();
    let ctl = OpCtl::new(None);
    ctl.cancel();
    let job = Job::new(ctl, Box::new(|_| {}), Box::new(|_| {}));
    let out = run(&FileOp::Copy { items: vec![fp(&big)], dest: fp(&dst) }, &job, None);
    let error = out.error.unwrap();
    assert_eq!(error.kind, flit_platform::PlatformErrorKind::Cancelled);
    assert_eq!(error.message, CANCELLED);
    assert!(listing(&dst).is_empty());
    let e = Engine::new(&job);
    let src = top(&fp(&big)).unwrap();
    let into = open_dir(&dst).unwrap();
    let dest = fp(&dst.join("big"));
    let r = e.copy_file(src.dir.as_raw_fd(), &src.name, &src.node, into.as_raw_fd(), c"big", Mode::Copy, &dest);
    assert_eq!(r.unwrap_err().raw_os_error(), Some(libc::ECANCELED));
    assert!(listing(&dst).is_empty(), "the half-written file is removed");
    write(&dst.join("big"), "old");
    assert!(e.replace_file((src.dir.as_raw_fd(), &src.name), (into.as_raw_fd(), c"big", &dest), &src.node, Mode::Copy).is_err());
    assert_eq!(listing(&dst), [b"big".to_vec()]);
    assert_eq!(read(&dst.join("big")), "old");
}

#[test]
fn replacing_a_linux_file_can_restore_both_versions_in_their_original_paths() {
    use flit_core::{provider::OpReport, undo::Step};
    use flit_platform::{replay, FileOps};

    struct Shell(Trash);
    impl FileOps for Shell {
        fn file_op(&mut self, op: &FileOp, job: &Job) -> OpOutcome {
            run(op, job, Some(&self.0))
        }

        fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
            unreachable!()
        }

        fn clear_clipboard(&mut self) {}

        fn can_restore(&self, backup: &FsPath, original: &FsPath) -> bool {
            let (Some(backup), Some(original)) = (host(backup), host(original)) else { return false };
            self.0.entry(backup).is_ok_and(|(_, found)| found.as_path() == original)
        }
    }

    for moving in [false, true] {
        let t = Temp::new("undo-linux-replace");
        let from = t.path().join("source/file");
        let to = t.path().join("destination/file");
        write(&from, "incoming");
        write(&to, "old");
        let dest = fp(to.parent().unwrap());
        let op = if moving { FileOp::Move { items: vec![fp(&from)], dest } } else { FileOp::Copy { items: vec![fp(&from)], dest } };
        let mut shell = Shell(Trash::new(t.path().join("data/Trash"), unsafe { libc::getuid() }));
        let job = quiet(Resolve::Replace);
        let report: OpReport = replay::report(op.clone(), ok(shell.file_op(&op, &job)), Some(undo_fs()), &job);
        assert_eq!(read(&to), "incoming");
        let Some(Step::Checked(plan)) = flit_core::undo::inverse(&report) else { panic!("expected a checked replacement") };
        let job = Job::new(OpCtl::checked(plan.clone()), Box::new(|_| {}), Box::new(|_| {}));
        let (result, _) = replay::run(&mut shell, &plan, Some(undo_fs()), &job);
        ok(result);
        assert_eq!(read(&from), "incoming");
        assert_eq!(read(&to), "old");
    }
}

#[test]
#[ignore]
fn bench_copy_a_thousand_small_files_and_one_large_file_against_cp() {
    let t = Temp::new("bench-copy");
    let small = t.path().join("small");
    for k in 0..1000 {
        std::fs::create_dir_all(&small).unwrap();
        t.file(&format!("small/f{k}"), 1024);
    }
    let mut runs = Vec::new();
    for k in 0..5 {
        let dest = t.path().join(format!("out{k}"));
        std::fs::create_dir_all(&dest).unwrap();
        let start = std::time::Instant::now();
        ok(run(&FileOp::Copy { items: vec![fp(&small)], dest: fp(&dest) }, &quiet(Resolve::Skip), None));
        runs.push(start.elapsed());
    }
    runs.sort();
    eprintln!("measure: 1000 x 1 KB copy median {:?}", runs[2]);
    let big = t.path().join("big");
    let mb = vec![7u8; 1 << 20];
    let mut f = std::fs::File::create(&big).unwrap();
    for _ in 0..1024 {
        std::io::Write::write_all(&mut f, &mb).unwrap();
    }
    drop(f);
    let ours = t.path().join("ours");
    std::fs::create_dir_all(&ours).unwrap();
    let start = std::time::Instant::now();
    ok(run(&FileOp::Copy { items: vec![fp(&big)], dest: fp(&ours) }, &quiet(Resolve::Skip), None));
    let flit = start.elapsed();
    let start = std::time::Instant::now();
    assert!(std::process::Command::new("cp").arg(&big).arg(t.path().join("by-cp")).status().unwrap().success());
    let cp = start.elapsed();
    eprintln!("measure: 1 GB copy flit {flit:?}, cp {cp:?}, flit speed {:.0}% of cp", cp.as_secs_f64() / flit.as_secs_f64() * 100.0);
}

#[test]
fn a_failing_item_asks_and_retry_skip_and_skip_all_work() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let t = Temp::new("ops-fail");
    let src = t.path().join("src");
    let names = ["a", "b", "c", "d", "e"];
    names.iter().for_each(|n| write(&src.join(n), n));
    let shut = |n: &str, mode| std::fs::set_permissions(src.join(n), std::fs::Permissions::from_mode(mode)).unwrap();
    ["a", "c", "d", "e"].iter().for_each(|n| shut(n, 0o000));
    let dst = t.path().join("dst");
    std::fs::create_dir(&dst).unwrap();
    let ctl = OpCtl::new(None);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (c, s, fix) = (ctl.clone(), seen.clone(), src.clone());
    let answers = Mutex::new(vec![(Recover::Retry, false), (Recover::Skip, false), (Recover::Skip, true)]);
    let job = Job::asking(
        ctl,
        Box::new(|_| {}),
        Box::new(|_| {}),
        Box::new(move |f: Failure| {
            if s.lock().unwrap().is_empty() {
                std::fs::set_permissions(fix.join("a"), std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            s.lock().unwrap().push(f.path);
            let (choice, all) = answers.lock().unwrap().remove(0);
            c.answer_failure(choice, all);
        }),
    );
    let items: Vec<FsPath> = names.iter().map(|n| fp(&src.join(n))).collect();
    let out = run(&FileOp::Copy { items: items.clone(), dest: fp(&dst) }, &job, None);
    ["c", "d", "e"].iter().for_each(|n| shut(n, 0o644));
    assert_eq!(*seen.lock().unwrap(), [items[0].clone(), items[2].clone(), items[3].clone()], "e is not asked about after Skip all");
    assert_eq!(out.items, [(items[0].clone(), fp(&dst.join("a"))), (items[1].clone(), fp(&dst.join("b")))], "Retry copies the item once its cause is gone");
    let e = out.error.expect("skipped items are summed up");
    assert!(e.message.starts_with("3 items failed") && e.code != libc::EACCES as i64, "{}", e.message);
}

#[test]
fn links_point_at_absolute_targets_get_numbered_names_and_undo_deletes_them() {
    flit_core::i18n::set_lang(flit_core::i18n::Lang::En);
    let t = Temp::new("ops-link");
    let file = t.file("report.txt", 4);
    let folder = t.path().join("docs");
    std::fs::create_dir(&folder).unwrap();
    let dest = t.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let op = FileOp::Link { items: vec![fp(&file), fp(&folder)], dest: fp(&dest) };
    let made = ok(run(&op, &quiet(Resolve::Skip), None));
    assert_eq!(made, [(fp(&file), fp(&dest.join("Link to report.txt"))), (fp(&folder), fp(&dest.join("Link to docs")))]);
    assert_eq!(std::fs::read_link(dest.join("Link to report.txt")).unwrap(), file);
    assert_eq!(std::fs::read_link(dest.join("Link to docs")).unwrap(), folder);
    assert!(std::fs::metadata(dest.join("Link to docs")).unwrap().is_dir());
    let again = ok(run(&op, &quiet(Resolve::Skip), None));
    assert_eq!(again, [(fp(&file), fp(&dest.join("Link to report (2).txt"))), (fp(&folder), fp(&dest.join("Link to docs (2)")))]);
    let odd = t.path().join(OsStr::from_bytes(b"x\xFF"));
    write(&odd, "x");
    let raw = ok(run(&FileOp::Link { items: vec![fp(&odd)], dest: fp(&dest) }, &quiet(Resolve::Skip), None));
    assert_eq!(raw[0].1.file_name(), b"Link to x\xFF");
    let missing = run(&FileOp::Link { items: vec![fp(&t.path().join("gone"))], dest: fp(&dest) }, &quiet(Resolve::Skip), None);
    assert!(missing.error.unwrap().message.ends_with("gone does not exist"));
    let report = flit_core::provider::OpReport::new(op, made);
    let Some(flit_core::undo::Step::Run(undo)) = flit_core::undo::inverse(&report) else { panic!("making links is undoable") };
    ok(run(&undo, &quiet(Resolve::Skip), None));
    assert!(std::fs::symlink_metadata(dest.join("Link to report.txt")).is_err() && std::fs::symlink_metadata(dest.join("Link to docs")).is_err());
    assert!(file.exists() && folder.is_dir(), "undo removes the links, never their targets");
    assert_eq!(listing(&dest).len(), 3);
}

#[test]
fn flatten_moves_nested_files_up_numbers_collisions_and_undoes() {
    let t = Temp::new("ops-flatten");
    let root = t.path().join("root");
    write(&root.join("top.txt"), "root");
    write(&root.join("a/one.txt"), "1");
    write(&root.join("a/b/two.txt"), "2");
    write(&root.join("c/top.txt"), "c");
    std::fs::create_dir_all(root.join("empty/deeper")).unwrap();
    symlink("../top.txt", root.join("a/b/link")).unwrap();
    let (job, seen) = answering(Resolve::KeepBoth, false);
    let out = run(&FileOp::Flatten(vec![fp(&root)]), &job, None);
    assert!(out.error.is_none(), "{:?}", out.error);
    let mut moved = out.items.clone();
    moved.sort_by(|a, b| a.1.as_bytes().cmp(b.1.as_bytes()));
    let pair = |from: &str, to: &str| (fp(&root.join(from)), fp(&root.join(to)));
    assert_eq!(moved, [pair("a/b/link", "link"), pair("a/one.txt", "one.txt"), pair("c/top.txt", "top (2).txt"), pair("a/b/two.txt", "two.txt")]);
    assert_eq!(seen.lock().unwrap().len(), 1, "the clash with top.txt is asked about");
    assert_eq!(listing(&root), [b"link".to_vec(), b"one.txt".to_vec(), b"top (2).txt".to_vec(), b"top.txt".to_vec(), b"two.txt".to_vec()]);
    assert_eq!((read(&root.join("top.txt")), read(&root.join("top (2).txt"))), ("root".into(), "c".into()));
    assert!(std::fs::symlink_metadata(root.join("link")).unwrap().file_type().is_symlink());
    let report = flit_core::provider::OpReport::new(FileOp::Flatten(vec![fp(&root)]), out.items);
    let Some(flit_core::undo::Step::Run(undo)) = flit_core::undo::inverse(&report) else { panic!("flatten is undoable") };
    ok(run(&undo, &quiet(Resolve::Skip), None));
    assert_eq!([read(&root.join("a/one.txt")), read(&root.join("a/b/two.txt")), read(&root.join("c/top.txt")), read(&root.join("top.txt"))], ["1", "2", "c", "root"]);
}

#[test]
fn flatten_skips_answers_and_leaves_folders_that_still_hold_files() {
    let t = Temp::new("ops-flatten-skip");
    let root = t.path().join("root");
    write(&root.join("same"), "top");
    write(&root.join("sub/same"), "inner");
    write(&root.join("sub/other"), "o");
    let out = run(&FileOp::Flatten(vec![fp(&root)]), &quiet(Resolve::Skip), None);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items, [(fp(&root.join("sub/other")), fp(&root.join("other")))]);
    assert_eq!(listing(&root.join("sub")), [b"same".to_vec()], "a skipped file keeps its folder");
    let file = run(&FileOp::Flatten(vec![fp(&root.join("same"))]), &quiet(Resolve::Skip), None);
    assert!(file.error.is_some() && file.items.is_empty());
}

#[test]
fn trash_failures_list_the_items_that_can_be_deleted_instead() {
    let t = Temp::new("ops-untrashable");
    let trash = Trash::new(t.path().join("data/Trash"), unsafe { libc::getuid() });
    let fine = t.file("fine", 1);
    let locked = t.path().join("locked");
    write(&locked.join("stuck"), "s");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let items = vec![fp(&fine), fp(&locked.join("stuck")), fp(&t.path().join("gone"))];
    let out = run(&FileOp::Trash(items), &quiet(Resolve::Skip), Some(&trash));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let root = unsafe { libc::geteuid() } == 0;
    assert_eq!(out.items.len(), if root { 2 } else { 1 });
    let error = out.error.unwrap();
    if root {
        assert!(error.untrashable.is_empty(), "root can trash the file in the read-only folder");
    } else {
        assert!(error.message.starts_with("2 items failed. Cannot move"), "{}", error.message);
        assert_eq!(error.untrashable, [fp(&locked.join("stuck"))], "only items that still exist can be deleted instead");
    }
    write(&locked.join("stuck"), "s");
    let nowhere = run(&FileOp::Trash(vec![fp(&locked.join("stuck"))]), &quiet(Resolve::Skip), None);
    assert_eq!(nowhere.error.unwrap().untrashable, [fp(&locked.join("stuck"))]);
    assert!(run(&FileOp::Trash(vec![fp(&locked)]), &quiet(Resolve::Skip), Some(&trash)).error.is_none());
}

#[test]
fn destination_file_systems_are_found_by_device_number() {
    let table = crate::mounts::parse(b"29 1 0:31 /@ / rw - btrfs /dev/nvme0n1p2 rw\n51 29 8:33 / /run/media/ann/USB rw - vfat /dev/sdc1 rw\n52 29 8:49 / /run/media/ann/Big rw - exfat /dev/sdd1 rw\n53 29 8:33 / /mnt/again rw - vfat /dev/sdc1 rw\n");
    assert_eq!(file_system(&table, 8 << 32 | 33).as_deref(), Some("vfat"));
    assert_eq!(file_system(&table, 8 << 32 | 49).as_deref(), Some("exfat"));
    assert_eq!(file_system(&table, 31).as_deref(), Some("btrfs"));
    assert_eq!(file_system(&table, 9 << 32 | 1), None);
    let t = Temp::new("ops-fs");
    let job = quiet(Resolve::Skip);
    let mut e = Engine::new(&job);
    let dir = open_dir(t.path()).unwrap();
    let real = e.dest_fs(dir.as_raw_fd());
    assert_eq!(real, crate::mounts::read().ok().and_then(|m| file_system(&m, node_at(dir.as_raw_fd(), c".", false).unwrap().dev)));
}

#[test]
fn fat_destinations_get_safe_names_and_refuse_files_over_four_gibibytes_up_front() {
    let t = Temp::new("ops-fat");
    let src = t.path().join("src");
    write(&src.join("a:b.txt"), "x");
    write(&src.join("fine.txt"), "y");
    write(&src.join("trailing."), "z");
    write(&src.join("sub/what?"), "q");
    write(&src.join("sub/ok"), "k");
    let dst = t.path().join("dst");
    std::fs::create_dir(&dst).unwrap();
    let dir = open_dir(&dst).unwrap();
    let dev = node_at(dir.as_raw_fd(), c".", false).unwrap().dev;
    let pretend = |fs: &str| Some((dev, Some(fs.to_owned())));
    let job = quiet(Resolve::Skip);
    let mut e = Engine::new(&job);
    e.dest_fs = pretend("vfat");
    e.copy(&[fp(&src)], &fp(&dst));
    assert_eq!(listing(&dst.join("src")), [b"a_b.txt".to_vec(), b"fine.txt".to_vec(), b"sub".to_vec(), b"trailing".to_vec()]);
    assert_eq!(listing(&dst.join("src/sub")), [b"ok".to_vec(), b"what_".to_vec()]);
    assert!(e.error().is_none(), "names the drive refuses are copied under safe names like Nautilus does");

    let mut e = Engine::new(&job);
    e.dest_fs = pretend("exfat");
    e.rename(&fp(&src.join("fine.txt")), b"CON.txt");
    assert!(e.first.as_deref().is_some_and(|m| m.ends_with("the name CON.txt, which is reserved for devices")), "{:?}", e.first);
    assert!(src.join("fine.txt").exists());
    e.rename(&fp(&src.join("fine.txt")), b"plain.txt");
    assert!(src.join("plain.txt").exists() && e.failed == 1);

    let huge = Node { mode: libc::S_IFREG | 0o644, size: 5 << 30, ..Node::default() };
    let small = Node { mode: libc::S_IFREG | 0o644, size: 4 << 20, ..Node::default() };
    let (big_src, big_dst) = (fp(&src.join("movie.mkv")), fp(&dst.join("movie.mkv")));
    let mut e = Engine::new(&job);
    e.dest_fs = pretend("vfat");
    assert!(e.fits(dir.as_raw_fd(), &big_src, &big_dst, &small));
    assert!(!e.fits(dir.as_raw_fd(), &big_src, &big_dst, &huge));
    assert!(e.first.as_deref().is_some_and(|m| m.contains("movie.mkv: the file is larger than")), "{:?}", e.first);
    let (asking, seen) = answering(Resolve::Skip, false);
    let mut e = Engine::new(&asking);
    e.dest_fs = pretend("msdos");
    assert!(!e.fits(dir.as_raw_fd(), &big_src, &big_dst, &huge));
    let asked = seen.lock().unwrap();
    assert!(asked.len() == 1 && flit_core::limits::too_large(&asked[0]) && asked[0].from.size == 5 << 30 && asked[0].to.size == flit_core::limits::FAT_MAX_FILE);
    assert!(e.first.is_none(), "an answered question is not an error");
    for fs in ["exfat", "ext4"] {
        let mut e = Engine::new(&job);
        e.dest_fs = pretend(fs);
        assert!(e.fits(dir.as_raw_fd(), &big_src, &big_dst, &huge), "{fs}");
    }
}

#[test]
fn errors_name_the_system_reason_once() {
    let t = Temp::new("ops-reason");
    let missing = fp(&t.path().join("missing"));
    let e = run(&FileOp::NewFolder { dir: missing.clone(), name: b"n".to_vec() }, &quiet(Resolve::Skip), None).error.unwrap();
    let expected = format!("{}: {}", missing.display(), flit_core::fmt::io_reason(&errno(libc::ENOENT)));
    assert_eq!(e.message, expected);
    assert_eq!(e.to_string(), expected, "no Windows-style code on Linux");
    assert!(!e.message.contains("os error"));
}

#[test]
fn read_only_drives_explain_themselves() {
    let t = Temp::new("ops-read-only");
    let p = fp(&t.path().join("f"));
    let rofs = errno(libc::EROFS);
    assert_eq!(failed_at(&p, &rofs), flit_core::tr_en!(S::ErrorsOpsExtraReadOnlyAt, path = p.display()));
    assert_eq!(reason(&p, &rofs), S::ErrorsOpsExtraReadOnly.en());
    assert_eq!(reason(&p, &errno(libc::EACCES)), flit_core::fmt::io_reason(&errno(libc::EACCES)));
}

#[test]
fn renaming_to_another_name_of_the_same_file_goes_through_a_temporary_name() {
    let t = Temp::new("ops-rename-case");
    let a = t.file("a.txt", 1);
    let out = run(&FileOp::Rename { item: fp(&a), name: b"A.txt".to_vec() }, &quiet(Resolve::Skip), None);
    assert_eq!(ok(out), [(fp(&a), fp(&t.path().join("A.txt")))]);
    std::fs::hard_link(t.path().join("A.txt"), t.path().join("b.txt")).unwrap();
    let out = run(&FileOp::Rename { item: fp(&t.path().join("A.txt")), name: b"b.txt".to_vec() }, &quiet(Resolve::Skip), None);
    assert!(out.error.unwrap().message.contains("already exists"), "a second link is a different name, not a case variant");
    assert_eq!(listing(t.path()), [b"A.txt".to_vec(), b"b.txt".to_vec()], "the first name is put back");
    let dir = open_dir(t.path()).unwrap();
    rename_through_temporary(dir.as_raw_fd(), c"A.txt", c"c.txt").unwrap();
    assert_eq!(listing(t.path()), [b"b.txt".to_vec(), b"c.txt".to_vec()]);
}

#[test]
fn copying_a_small_file_writes_it_without_reporting_unflushed_bytes() {
    let t = Temp::new("ops-small-writeback");
    let source = t.file("source", 32 * 1024);
    let dest = t.path().join("dest");
    let from = std::fs::File::open(&source).unwrap();
    let to = std::fs::File::create(&dest).unwrap();
    let job = Job::detached(Resolve::Replace);
    let engine = Engine::new(&job);

    assert_eq!(engine.copy_data(from.as_raw_fd(), to.as_raw_fd(), false, writeback::CopyMode::Deferred).unwrap(), None);
    assert_eq!(read(&source), read(&dest));
    assert_eq!(job.progress().bytes, 0);
}

#[test]
fn copy_fallback_reuses_its_buffer_and_writes_only_the_bytes_read() {
    let temp = Temp::new("ops-copy-buffer");
    let job = quiet(Resolve::Replace);
    let engine = Engine::new(&job);
    let copy = |name: &str, payload: &[u8]| {
        let (from, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        writer.write_all(payload).unwrap();
        drop(writer);
        let path = temp.path().join(name);
        let to = std::fs::File::create(&path).unwrap();
        engine.copy_data(from.as_raw_fd(), to.as_raw_fd(), false, writeback::CopyMode::Deferred).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), payload);
    };
    copy("longer", b"this is the larger payload");
    assert_eq!(engine.copy_buffer.borrow().len(), BUFFER);
    let first_buffer = engine.copy_buffer.borrow().as_ptr();
    copy("shorter", b"new");
    assert_eq!(engine.copy_buffer.borrow().as_ptr(), first_buffer);
}

#[test]
fn a_linux_move_uses_the_resolved_primary_name_and_skip_keeps_the_group_at_source() {
    let temp = Temp::new("ops-companions");
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let primary = source.join("photo.jpg");
    let sidecar = source.join("photo.jpg.xmp");
    write(&primary, "photo");
    write(&sidecar, "metadata");
    write(&destination.join("photo.jpg"), "existing");
    write(&destination.join("photo.jpg.xmp"), "existing metadata");
    let op = FileOp::Move { items: vec![fp(&primary)], dest: fp(&destination) };
    let (skip, questions) = answering(Resolve::Skip, false);
    let skipped = skip.prepare_companions(&crate::LinuxFs, op.clone()).unwrap();
    assert!(ok(run(&skipped, &skip, None)).is_empty());
    assert_eq!(questions.lock().unwrap().len(), 1);
    assert!(primary.exists() && sidecar.exists());

    let (rename, questions) = answering(Resolve::KeepBoth, false);
    let renamed = rename.prepare_companions(&crate::LinuxFs, op).unwrap();
    assert_eq!(ok(run(&renamed, &rename, None)).len(), 2);
    assert_eq!(questions.lock().unwrap().len(), 1);
    assert_eq!(read(&destination.join("photo (2).jpg")), "photo");
    assert_eq!(read(&destination.join("photo (2).jpg.xmp")), "metadata");
    assert_eq!(read(&destination.join("photo.jpg.xmp")), "existing metadata");
    assert!(!primary.exists() && !sidecar.exists());
}
