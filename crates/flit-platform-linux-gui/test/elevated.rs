use super::*;
use crate::temp::Temp;
use flit_core::provider::{OpCtl, Resolve, Template};
use std::os::unix::fs::PermissionsExt;
use flit_platform::Provider;

fn fs_path(p: &Path) -> FsPath {
    FsPath::parse_exact(p.as_os_str().as_bytes()).unwrap()
}

#[test]
fn missing_administrator_helpers_name_their_packages_without_changing_access() {
    let t = Temp::new("missing-admin");
    let dir = fs_path(t.path());
    let op = FileOp::NewFolder { dir: dir.clone(), name: b"untouched".to_vec() };
    crate::temp::with_tools(&[], || {
        let missing = crate::shell::tool_missing("pkexec").message;
        let error = run(&op, &Job::detached(Resolve::Skip)).unwrap_err();
        assert_eq!(error.message, missing);
        crate::temp::assert_missing(&error.message, "pkexec");
        let error = grant_access(&dir).error.unwrap();
        assert_eq!(error.message, missing);
        crate::temp::assert_missing(&error.message, "pkexec");
    });
    crate::temp::with_tools(&["pkexec"], || {
        let error = grant_access(&dir).error.unwrap();
        assert_eq!(error.message, crate::shell::tool_missing("setfacl").message);
        crate::temp::assert_missing(&error.message, "setfacl");
        let error = run(&op, &Job::detached(Resolve::Skip)).unwrap_err();
        assert!(!error.message.contains("Install the package"), "{}", error.message);
    });
    crate::temp::with_tools(&["pkexec", "setfacl"], || {
        assert!(grant_access(&dir).error.is_none());
    });
    assert!(!t.path().join("untouched").exists());
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
    let why = crate::shell::tool_missing("pkexec").message;
    let out = retried(first(), Err(unelevated(&why)));
    assert_eq!(out.items, [(a.clone(), a.clone())]);
    let e = out.error.unwrap();
    assert_eq!(e.code, libc::EACCES as i64);
    assert_eq!(e.message, flit_core::tr_en!(S::ErrorsOpsExtraNotElevated, error = denied().message, why = why));
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

fn fake_pkexec(t: &Temp, body: &str) {
    let script = t.path().join("pkexec");
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn launch_error(launcher: &RootLauncher) -> String {
    let mut process = launcher.launch().unwrap();
    let mut buf = [0u8; 4];
    let error = process.output.read(&mut buf).unwrap_err().to_string();
    (process.stop)(Duration::ZERO);
    error
}

fn system_program() -> PathBuf {
    std::fs::canonicalize("/bin/sh").unwrap()
}

#[test]
fn the_root_agent_runs_a_root_owned_flit_by_its_real_path_in_agent_mode_only() {
    let t = Temp::new("root-agent-args");
    fake_pkexec(&t, "echo \"$@\" >&2\nexit 127");
    let error = crate::temp::with_tool_dir(t.path(), || launch_error(&RootLauncher::with(|| root_agent_command(Path::new("/bin/sh")))));
    let expected = format!("The administrator agent could not start: {} {FLAG} {}", system_program().display(), flit_core::i18n::lang().code());
    assert_eq!(error, expected, "a 127 without a polkit refusal is a start failure, not a wrong password");
}

#[test]
fn pkexec_codes_tell_a_dismissed_prompt_a_refusal_and_a_start_failure_apart() {
    assert_eq!(refusal(Some(126), ""), "the password prompt was dismissed");
    let denied = "Error executing command as another user: Not authorized\n\nThis incident has been reported.";
    assert!(refusal(Some(127), denied).starts_with("polkit refused: Error executing command as another user: Not authorized"));
    assert_eq!(refusal(Some(127), "Cannot run program /tmp/.mount_x/flit: No such file or directory"), "The administrator agent could not start: Cannot run program /tmp/.mount_x/flit: No such file or directory");
    assert_eq!(refusal(Some(127), ""), "the administrator agent did not answer");
    assert_eq!(refusal(Some(1), "boom"), "The elevated helper stopped: boom");
}

#[test]
fn a_dismissed_password_prompt_is_explained_and_the_tab_is_denied() {
    let t = Temp::new("root-agent-dismissed");
    fake_pkexec(&t, "exit 126");
    let error = crate::temp::with_tool_dir(t.path(), || launch_error(&RootLauncher::with(|| root_agent_command(Path::new("/bin/sh")))));
    assert_eq!(error, "the password prompt was dismissed");
    let (tx, events) = crossbeam_channel::unbounded();
    let launcher = RootLauncher::with(|| {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "echo oops >&2; exit 3"]);
        Ok(c)
    });
    let local: Arc<dyn flit_platform::Provider> = Arc::new(Nowhere);
    let admin = flit_platform::WslProvider::admin(Some(Box::new(launcher)), local, tx, Arc::new(|| {}), flit_platform::Timing::default(), flit_platform::Clipboard::default()).unwrap();
    admin.submit(flit_core::ProviderRequest::Watch { tab: 1, gen: 1, path: Some(FsPath::parse("/root").unwrap()) });
    let (slot, token) = (Arc::new(flit_core::provider::SnapshotSlot::default()), flit_core::provider::GenToken::default());
    token.set(1);
    admin.submit(flit_core::ProviderRequest::List { tab: 1, gen: 1, path: FsPath::parse("/root").unwrap(), view: Default::default(), case: flit_core::sort::CaseMode::Sensitive, out: slot, cancel: token });
    let end = std::time::Instant::now() + Duration::from_secs(10);
    let denied = loop {
        match events.recv_timeout(end.saturating_duration_since(std::time::Instant::now())).expect("a denial") {
            flit_core::ProviderEvent::Denied { tab: 1, error } => break error,
            _ => continue,
        }
    };
    assert_eq!(denied, "Cannot open the folder as administrator: The elevated helper stopped: oops");
}

struct Nowhere;

impl flit_platform::Provider for Nowhere {
    fn submit(&self, _: flit_core::ProviderRequest) {
        panic!("a root tab never falls back to the user");
    }
}

#[test]
fn without_pkexec_the_root_agent_names_the_package() {
    let error = crate::temp::with_tools(&[], || RootLauncher::pkexec().launch().err().unwrap().to_string());
    assert_eq!(error, crate::shell::tool_missing("pkexec").message);
    crate::temp::assert_missing(&error, "pkexec");
}

#[test]
fn a_flit_that_belongs_to_the_user_never_reaches_pkexec() {
    let t = Temp::new("root-agent-user-flit");
    fake_pkexec(&t, "touch \"$(dirname \"$0\")/ran\"; exit 0");
    let exe = t.file("flit", 4);
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let unsafe_text = S::ErrorsEngineAdminUnsafe.en();
    if unsafe { libc::geteuid() } != 0 {
        let error = crate::temp::with_tool_dir(t.path(), || RootLauncher::with(move || root_agent_command(&exe)).launch().err().unwrap().to_string());
        assert_eq!(error, unsafe_text, "a user-owned flit, like ~/.local/bin/flit from install.sh, is refused");
        let own = crate::temp::with_tool_dir(t.path(), || RootLauncher::pkexec().launch().err().unwrap().to_string());
        assert_eq!(own, unsafe_text, "the test binary belongs to the user too");
        assert!(!t.path().join("ran").exists(), "pkexec was never started");
    }
    assert_eq!(agent_program(&system_program()).unwrap(), system_program());
    assert_eq!(agent_program(Path::new("/bin/sh")).unwrap(), system_program(), "the real path is used, not the link");
    let program = S::ErrorsEngineAdminProgram.en();
    assert_eq!(agent_program(Path::new("flit")).unwrap_err().to_string(), program, "a bare name would be looked up in $PATH");
    assert_eq!(agent_program(&t.path().join("flit (deleted)")).unwrap_err().to_string(), program);
    assert_eq!(agent_program(Path::new("/usr/bin")).unwrap_err().to_string(), program);
}

type Node = (&'static str, u32, u32, bool);

fn tree(nodes: &'static [Node]) -> impl Fn(&Path) -> io::Result<Owned> {
    move |p: &Path| nodes.iter().find(|n| Path::new(n.0) == p).map(|n| Owned { uid: n.1, mode: n.2, dir: n.3 }).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
}

#[test]
fn only_root_owned_files_in_root_owned_folders_nobody_else_can_write_are_trusted() {
    let system = tree(&[("/", 0, 0o40755, true), ("/usr", 0, 0o40755, true), ("/usr/bin", 0, 0o40755, true), ("/usr/bin/flit", 0, 0o100755, false)]);
    assert!(root_only(Path::new("/usr/bin/flit"), &system));
    let user_file = tree(&[("/", 0, 0o40755, true), ("/usr", 0, 0o40755, true), ("/usr/bin", 0, 0o40755, true), ("/usr/bin/flit", 1000, 0o100755, false)]);
    assert!(!root_only(Path::new("/usr/bin/flit"), &user_file), "a file of the user");
    let user_dir = tree(&[("/", 0, 0o40755, true), ("/home", 0, 0o40755, true), ("/home/u", 1000, 0o40700, true), ("/home/u/bin", 0, 0o40755, true), ("/home/u/bin/flit", 0, 0o100755, false)]);
    assert!(!root_only(Path::new("/home/u/bin/flit"), &user_dir), "a folder of the user on the path can swap the file");
    let open_dir = tree(&[("/", 0, 0o40755, true), ("/opt", 0, 0o40777, true), ("/opt/flit", 0, 0o100755, false)]);
    assert!(!root_only(Path::new("/opt/flit"), &open_dir), "a root file in a folder everybody can write");
    let sticky = tree(&[("/", 0, 0o40755, true), ("/tmp", 0, 0o41777, true), ("/tmp/flit", 0, 0o100755, false)]);
    assert!(!root_only(Path::new("/tmp/flit"), &sticky), "a sticky folder everybody can write is not trusted either");
    let group = tree(&[("/", 0, 0o40755, true), ("/usr", 0, 0o40755, true), ("/usr/flit", 0, 0o100775, false)]);
    assert!(!root_only(Path::new("/usr/flit"), &group), "a file the group can write");
    let missing = tree(&[("/usr/bin/flit", 0, 0o100755, false)]);
    assert!(!root_only(Path::new("/usr/bin/flit"), &missing));
}

#[test]
fn the_one_off_elevation_refuses_a_flit_of_the_user_and_an_untrusted_setfacl() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let t = Temp::new("one-off-user-flit");
    fake_pkexec(&t, "touch \"$(dirname \"$0\")/ran\"; exit 0");
    let dir = fs_path(t.path());
    let op = FileOp::NewFolder { dir: dir.clone(), name: b"x".to_vec() };
    let error = crate::temp::with_tool_dir(t.path(), || run(&op, &Job::detached(Resolve::Skip)).unwrap_err());
    assert_eq!(error.message, S::ErrorsEngineAdminUnsafe.en());
    let setfacl = t.path().join("setfacl");
    std::fs::write(&setfacl, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&setfacl, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = crate::temp::with_tool_dir(t.path(), || grant_access(&dir).error.unwrap());
    assert_eq!(error.message, "setfacl is not a system program that belongs to root, so Flit does not run it as administrator");
    assert!(!t.path().join("ran").exists(), "pkexec was never started");
}

#[test]
fn a_fake_pkexec_of_the_user_ahead_in_path_is_never_asked_for_the_password() {
    let t = Temp::new("fake-pkexec-first");
    fake_pkexec(&t, "touch \"$(dirname \"$0\")/ran\"; exit 0");
    let dirs = std::env::join_paths([t.path(), Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
    let refused = "pkexec is not a system program that belongs to root, so Flit does not run it as administrator";
    let (agent, one_off, grant) = crate::temp::with_tool_dir(Path::new(&dirs), || {
        crate::temp::with_real_owners(|| {
            let agent = RootLauncher::with(|| root_agent_command(Path::new("/bin/sh"))).launch().err().unwrap().to_string();
            let op = FileOp::NewFolder { dir: fs_path(t.path()), name: b"x".to_vec() };
            let one_off = run(&op, &Job::detached(Resolve::Skip)).unwrap_err().message;
            let grant = grant_access(&fs_path(t.path())).error.unwrap().message;
            (agent, one_off, grant)
        })
    });
    assert_eq!(agent, refused);
    assert_eq!(grant, refused);
    if unsafe { libc::geteuid() } != 0 {
        assert!(one_off == refused || one_off == S::ErrorsEngineAdminUnsafe.en(), "{one_off}");
    }
    assert!(!t.path().join("ran").exists(), "the fake pkexec never ran");
}

#[test]
fn only_a_setuid_pkexec_of_root_is_trusted() {
    let setuid = tree(&[("/", 0, 0o40755, true), ("/usr", 0, 0o40755, true), ("/usr/bin", 0, 0o40755, true), ("/usr/bin/pkexec", 0, 0o104755, false)]);
    assert!(setuid_root(Path::new("/usr/bin/pkexec"), &setuid));
    let plain = tree(&[("/", 0, 0o40755, true), ("/usr", 0, 0o40755, true), ("/usr/bin", 0, 0o40755, true), ("/usr/bin/pkexec", 0, 0o100755, false)]);
    assert!(!setuid_root(Path::new("/usr/bin/pkexec"), &plain), "without the setuid bit it is not the real pkexec");
    let user = tree(&[("/", 0, 0o40755, true), ("/home", 0, 0o40755, true), ("/home/u", 1000, 0o40755, true), ("/home/u/bin", 1000, 0o40755, true), ("/home/u/bin/pkexec", 1000, 0o104755, false)]);
    assert!(!setuid_root(Path::new("/home/u/bin/pkexec"), &user));
    if let Ok(real) = std::fs::canonicalize("/usr/bin/pkexec") {
        assert!(setuid_root(&real, &owned), "the installed pkexec passes");
    }
}
