use super::*;
use crate::temp::Temp;
use flit_core::provider::{Failure, OpCtl, Recover};
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
    let report = flit_core::provider::OpReport { op, items: made };
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
    let report = flit_core::provider::OpReport { op: FileOp::Flatten(vec![fp(&root)]), items: out.items };
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
    assert_eq!(out.items.len(), 1);
    let error = out.error.unwrap();
    if unsafe { libc::geteuid() } != 0 {
        assert!(error.message.starts_with("2 items failed. Cannot move"), "{}", error.message);
        assert_eq!(error.untrashable, [fp(&locked.join("stuck"))], "only items that still exist can be deleted instead");
    }
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
