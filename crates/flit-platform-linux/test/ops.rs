use super::*;
use crate::temp::Temp;
use flit_core::provider::OpCtl;
use std::ffi::OsStr;
use std::os::unix::fs::{symlink, FileTypeExt, MetadataExt, PermissionsExt};
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
    }
    assert_eq!(read(&t.path().join("keep/precious")), "p");
    let out = run(&FileOp::Delete(vec![fp(&doomed)]), &quiet(Resolve::Skip), None);
    assert_eq!(ok(out), [(fp(&doomed), fp(&doomed))]);
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
    assert_eq!(out.error.unwrap().message, CANCELLED);
    assert!(listing(&dst).is_empty());
    let e = Engine::new(&job);
    let src = top(&fp(&big)).unwrap();
    let into = open_dir(&dst).unwrap();
    let r = e.copy_file(src.dir.as_raw_fd(), &src.name, &src.node, into.as_raw_fd(), c"big", false);
    assert_eq!(r.unwrap_err().raw_os_error(), Some(libc::ECANCELED));
    assert!(listing(&dst).is_empty(), "the half-written file is removed");
    write(&dst.join("big"), "old");
    assert!(e.copy_file(src.dir.as_raw_fd(), &src.name, &src.node, into.as_raw_fd(), c"big", true).is_err());
    assert_eq!(listing(&dst), [b"big".to_vec()]);
    assert_eq!(read(&dst.join("big")), "old");
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
