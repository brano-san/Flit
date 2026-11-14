use super::*;
use crate::temp::Temp;
use flit_core::provider::{OpCtl, Resolve, Template};
use std::os::unix::fs::PermissionsExt;

fn fs_path(p: &Path) -> FsPath {
    FsPath::parse_exact(p.as_os_str().as_bytes()).unwrap()
}

fn locked(t: &Temp) -> PathBuf {
    let dir = t.path().join("locked");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("inside"), b"x").unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    dir
}

fn unlock(dir: &Path) {
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn operations_that_write_into_folders_the_user_cannot_change_ask_for_root_first() {
    if is_root() {
        return;
    }
    let t = Temp::new("elevate-need");
    let dir = locked(&t);
    let free = t.file("free.txt", 3);
    let new_folder = |d: &Path| FileOp::NewFolder { dir: fs_path(d), name: b"n".to_vec() };
    assert!(needs_root(&new_folder(&dir)));
    assert!(!needs_root(&new_folder(t.path())));
    assert!(needs_root(&FileOp::Copy { items: vec![fs_path(&free)], dest: fs_path(&dir.join("not yet")) }), "a missing destination is judged by its nearest folder");
    assert!(needs_root(&FileOp::Delete(vec![fs_path(&dir.join("inside"))])));
    assert!(needs_root(&FileOp::Link { items: vec![fs_path(&free)], dest: fs_path(&dir) }), "links are made with root rights in a locked folder");
    assert!(!needs_root(&FileOp::Link { items: vec![fs_path(&dir.join("inside"))], dest: fs_path(t.path()) }));
    assert!(needs_root(&FileOp::Move { items: vec![fs_path(&dir.join("inside"))], dest: fs_path(t.path()) }), "moving out needs write access to the source folder");
    assert!(!needs_root(&FileOp::Trash(vec![fs_path(&dir.join("inside"))])), "the Trash is never used as root");
    assert!(!needs_root(&FileOp::NewFile { dir: fs_path(t.path()), name: b"f".to_vec(), template: Template::Empty }));
    unlock(&dir);
}

#[test]
fn only_permission_failures_are_retried_and_finished_deletions_are_not_repeated() {
    let t = Temp::new("elevate-retry");
    let (a, b) = (fs_path(&t.path().join("a")), fs_path(&t.path().join("b")));
    let denied = |items: Vec<(FsPath, FsPath)>| OpOutcome { items, error: Some(PlatformError::new(libc::EACCES as i64, "denied")) };
    let delete = FileOp::Delete(vec![a.clone(), b.clone()]);
    if is_root() {
        assert_eq!(retry(&delete, &denied(Vec::new())), None);
        return;
    }
    assert_eq!(retry(&delete, &denied(Vec::new())), Some(delete.clone()));
    assert_eq!(retry(&delete, &denied(vec![(a.clone(), a.clone())])), Some(FileOp::Delete(vec![b.clone()])));
    assert_eq!(retry(&delete, &denied(vec![(a.clone(), a.clone()), (b.clone(), b.clone())])), None);
    let copy = FileOp::Copy { items: vec![a.clone(), b.clone()], dest: fs_path(t.path()) };
    assert_eq!(retry(&copy, &denied(vec![(a.clone(), a.clone())])), None, "a copy that was partly done is not run twice");
    assert_eq!(retry(&FileOp::Trash(vec![a.clone()]), &denied(Vec::new())), None);
    let other = OpOutcome { items: Vec::new(), error: Some(PlatformError::new(libc::ENOSPC as i64, "full")) };
    assert_eq!(retry(&delete, &other), None);
    assert_eq!(retry(&delete, &OpOutcome { items: Vec::new(), error: None }), None);
}

#[test]
fn a_permission_failure_in_the_engine_carries_the_access_code() {
    if is_root() {
        return;
    }
    let t = Temp::new("elevate-code");
    let dir = locked(&t);
    let out = flit_platform_linux::run_op(&FileOp::NewFolder { dir: fs_path(&dir), name: b"n".to_vec() }, &Job::detached(Resolve::Skip), None);
    assert_eq!(out.error.map(|e| e.code), Some(libc::EACCES as i64));
    unlock(&dir);
}

#[test]
fn the_helper_runs_the_operation_and_asks_back_about_conflicts() {
    let t = Temp::new("elevate-drive");
    let src = t.path().join("src");
    let dest = t.path().join("dest");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(src.join("note.txt"), b"new").unwrap();
    std::fs::write(dest.join("note.txt"), b"old").unwrap();
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (ev_r, ev_w) = std::io::pipe().unwrap();
    let agent = std::thread::spawn(move || flit_platform_linux::agent::serve(req_r, ev_w, "helper".into(), None));
    let ctl = OpCtl::new(None);
    let answer = ctl.clone();
    let asked = Arc::new(AtomicBool::new(false));
    let seen = asked.clone();
    let job = Job::new(
        ctl,
        Box::new(|_| {}),
        Box::new(move |_| {
            seen.store(true, Ordering::Release);
            answer.answer(Resolve::Replace, false);
        }),
    );
    let op = FileOp::Copy { items: vec![fs_path(&src.join("note.txt"))], dest: fs_path(&dest) };
    let out = drive(ev_r, req_w, &op, &job).unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);
    assert!(asked.load(Ordering::Acquire), "the conflict reached this side");
    assert_eq!(std::fs::read(dest.join("note.txt")).unwrap(), b"new");
    assert_eq!(out.items, [(fs_path(&src.join("note.txt")), fs_path(&dest.join("note.txt")))]);
    assert!(agent.join().unwrap().is_ok(), "the helper stops after the shutdown request");
}

#[test]
fn permission_changes_ask_for_root_only_for_items_of_other_users_or_foreign_owners() {
    if is_root() {
        return;
    }
    let t = Temp::new("elevate-access");
    let mine = fs_path(&t.file("mine", 1));
    let theirs = fs_path(Path::new("/etc/passwd"));
    let set = |items: Vec<FsPath>, edit: flit_core::provider::AccessEdit| FileOp::SetAccess { items, edit };
    let plus_x = flit_core::provider::AccessEdit { mode: flit_core::provider::ModeEdit { set: 0o100, clear: 0 }, ..Default::default() };
    assert!(!access_needs_root(&set(vec![mine.clone()], plus_x)));
    assert!(access_needs_root(&set(vec![theirs.clone()], plus_x)), "a file of another user needs root");
    assert!(access_needs_root(&set(vec![mine.clone()], flit_core::provider::AccessEdit { owner: Some(0), ..Default::default() })), "giving a file away needs root");
    let denied = OpOutcome { items: Vec::new(), error: Some(PlatformError::new(libc::EACCES as i64, "denied")) };
    assert_eq!(access_retry(&set(vec![mine.clone()], plus_x), &denied), Some(set(vec![mine], plus_x)));
}

#[test]
fn the_helper_speaks_the_language_of_the_window() {
    assert_eq!(spoken(Some(OsStr::new("ru"))), Some(Lang::Ru));
    assert_eq!(spoken(Some(OsStr::new("zh-CN"))), Some(Lang::ZhCn));
    assert_eq!(spoken(Some(OsStr::new("--bogus"))), None);
    assert_eq!(spoken(None), None);
    let args = helper_args(Path::new("/usr/bin/flit"));
    assert_eq!(args[..2], [OsString::from("/usr/bin/flit"), OsString::from(FLAG)]);
    assert_eq!(spoken(Some(&args[2])), Some(flit_core::i18n::lang()), "links made as root get names in the language of the window");
}

#[test]
fn a_retry_that_cannot_get_administrator_rights_says_why() {
    let (a, b) = (fs_path(Path::new("/x/a")), fs_path(Path::new("/x/b")));
    let denied = || PlatformError::new(libc::EACCES as i64, "/x/b: Permission denied");
    let first = || OpOutcome { items: vec![(a.clone(), a.clone())], error: Some(denied()) };
    let out = retried(first(), Err(unelevated(S::ErrorsOpsExtraNoPkexec.en())));
    assert_eq!(out.items, [(a.clone(), a.clone())]);
    let e = out.error.unwrap();
    assert_eq!(e.code, libc::EACCES as i64);
    assert_eq!(e.message, flit_core::tr_en!(S::ErrorsOpsExtraNotElevated, error = denied().message, why = S::ErrorsOpsExtraNoPkexec.en()));
    let out = retried(first(), Ok(OpOutcome { items: vec![(b.clone(), b.clone())], error: None }));
    assert_eq!((out.items, out.error.is_none()), (vec![(a.clone(), a.clone()), (b.clone(), b)], true));
    let up_front = only_elevated(unelevated(S::ErrorsOpsExtraAdminDismissed.en())).error.unwrap();
    assert_eq!(up_front.message, flit_core::tr_en!(S::ErrorsOpsExtraNeedsAdmin, why = S::ErrorsOpsExtraAdminDismissed.en()));
    let stopped = PlatformError::new(-1, flit_core::tr_en!(S::ErrorsOpsExtraAdminStopped, why = "broken pipe"));
    assert_eq!(only_elevated(stopped.clone()).error.unwrap().message, stopped.message, "a helper that ran and stopped is not a missing right");
}

#[test]
fn continue_on_a_locked_folder_lets_the_user_list_and_enter_it_and_nothing_more() {
    let args: Vec<String> = grant_args(1000, Path::new("/srv/locked dir")).into_iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args, ["-m", "u:1000:rX", "--", "/srv/locked dir"]);
    assert_eq!(grant_failure(Some(PKEXEC_DISMISSED), "").message, S::ErrorsOpsExtraAdminDismissed.en());
    let other = grant_failure(Some(1), "setfacl: /srv/x: Operation not supported\n");
    assert_eq!(other.message, "setfacl: setfacl: /srv/x: Operation not supported");
}
