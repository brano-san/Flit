use super::*;
use flit_core::git::GitStatus;
use std::fs;

pub(super) fn isolate(c: &mut Command) {
    static CONFIG: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let empty = CONFIG.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("flit-git-empty-{}.config", std::process::id()));
        fs::write(&path, "").unwrap();
        path
    });
    c.env("GIT_CONFIG_GLOBAL", empty).env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Ann").env("GIT_AUTHOR_EMAIL", "ann@example.com")
        .env("GIT_COMMITTER_NAME", "Ann").env("GIT_COMMITTER_EMAIL", "ann@example.com");
}

#[test]
fn sync_pulls_before_pushing_and_keeps_the_push_upstream_fallback() {
    let mut calls = Vec::new();
    let result = sync(&mut |args| {
        calls.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        if args == ["push"] { Err("fatal: The current branch topic/x has no upstream branch.".into()) } else { Ok(()) }
    });
    assert_eq!(result, Ok(()));
    assert_eq!(calls, [vec!["pull"], vec!["push"], vec!["push", "--set-upstream", "origin", "HEAD"]]);
}

#[test]
fn sync_never_pushes_after_a_failed_pull() {
    let mut calls = Vec::new();
    let result = sync(&mut |args| {
        calls.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        Err("CONFLICT: merge failed".into())
    });
    assert_eq!(result, Err("CONFLICT: merge failed".into()));
    assert_eq!(calls, [vec!["pull"]]);
}

#[test]
fn sync_reports_push_rejection_without_retrying_as_a_new_upstream() {
    let mut calls = Vec::new();
    let result = sync(&mut |args| {
        calls.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        if args == ["push"] { Err("error: failed to push some refs".into()) } else { Ok(()) }
    });
    assert_eq!(result, Err("error: failed to push some refs".into()));
    assert_eq!(calls, [vec!["pull"], vec!["push"]]);
}

#[test]
fn upstream_status_tracks_the_current_branch_and_detached_or_unborn_heads() {
    assert!(have_git());
    let t = temp("upstream папка с пробелами");
    let r = &t.0;
    assert!(status(r, false, &mut Cache::default()).is_none());
    sh(r, &["init", "-q"]);
    assert!(status(r, false, &mut Cache::default()).unwrap().upstream.is_none());
    fs::write(r.join("a.txt"), "1").unwrap();
    sh(r, &["add", "."]);
    sh(r, &["commit", "-qm", "first"]);
    sh(r, &["switch", "-c", "topic/x"]);
    assert!(status(r, false, &mut Cache::default()).unwrap().upstream.is_none());
    sh(r, &["branch", "--set-upstream-to=main"]);
    assert_eq!(status(r, false, &mut Cache::default()).unwrap().upstream.as_deref(), Some("main"));
    sh(r, &["switch", "--detach"]);
    assert!(status(r, false, &mut Cache::default()).unwrap().upstream.is_none());
}

#[test]
fn status_in_a_subfolder_shows_divergence_and_diff_and_stash_actions_use_the_selected_entry() {
    let t = temp("divergence");
    let bare = t.0.join("bare.git");
    let main = t.0.join("main");
    let peer = t.0.join("peer");
    sh(&t.0, &["init", "--bare", "-q", bare.to_str().unwrap()]);
    sh(&t.0, &["init", "-q", main.to_str().unwrap()]);
    let file = main.join("a.txt");
    fs::write(&file, "base\n").unwrap();
    sh(&main, &["add", "."]);
    sh(&main, &["commit", "-qm", "base"]);
    sh(&main, &["remote", "add", "origin", bare.to_str().unwrap()]);
    sh(&main, &["push", "-u", "origin", "main"]);
    sh(&t.0, &["clone", "--", bare.to_str().unwrap(), peer.to_str().unwrap()]);
    sh(&main, &["commit", "--allow-empty", "-qm", "local"]);
    sh(&peer, &["commit", "--allow-empty", "-qm", "remote"]);
    sh(&peer, &["push", "origin", "main"]);
    sh(&main, &["fetch", "origin"]);
    fs::write(&file, "saved\n").unwrap();
    sh(&main, &["stash", "push", "-m", "My stash"]);
    let nested = main.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(&file, "changed\nline added\n").unwrap();
    let info = status(&nested, false, &mut Cache::default()).unwrap();
    assert_eq!(info.upstream.as_deref(), Some("origin/main"));
    assert_eq!((info.ahead, info.behind, info.insertions, info.deletions), (1, 1, 2, 1));
    assert!(info.status.is_empty());
    assert_eq!(info.stashes.len(), 1);
    assert!(info.stashes[0].flit_branch().is_none());
    let oid = info.stashes[0].oid.clone();
    fs::write(&file, "base\n").unwrap();
    run_op(&main, &GitOp::ApplyStash(oid.clone())).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "saved\n");
    assert_eq!(stashes(&main).len(), 1);
    run_op(&main, &GitOp::DropStash(oid.clone())).unwrap();
    assert!(stashes(&main).is_empty());
    assert!(run_op(&main, &GitOp::DropStash(oid)).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "saved\n");
}

struct Temp(PathBuf);

#[test]
fn git_shortstat_counts_staged_lines_and_keeps_unborn_diff_semantics() {
    let temp = temp("staged shortstat");
    let dir = &temp.0;
    let file = dir.join("test.txt");
    sh(dir, &["init", "-q"]);
    fs::write(&file, "base\n").unwrap();
    sh(dir, &["add", "."]);
    let mut cache = Cache::default();
    let counts = |cache: &mut Cache| {
        let info = status(dir, false, cache).unwrap();
        (info.insertions, info.deletions)
    };
    assert_eq!(counts(&mut cache), (0, 0));
    sh(dir, &["commit", "-qm", "base"]);
    assert_eq!(counts(&mut cache), (0, 0));
    fs::write(&file, "updated\nsecond\n").unwrap();
    sh(dir, &["add", "."]);
    assert_eq!(counts(&mut cache), (2, 1));
    fs::write(&file, "updated\nsecond\nthird\n").unwrap();
    assert_eq!(counts(&mut cache), (3, 1));
}

#[test]
fn git_stash_status_reuses_the_reflog_cache_and_refreshes_it_when_the_log_changes() {
    let temp = temp("stash cache");
    let dir = &temp.0;
    let file = dir.join("test.txt");
    sh(dir, &["init", "-q"]);
    fs::write(&file, "base\n").unwrap();
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-qm", "base"]);
    let mut cache = Cache::default();
    assert!(status(dir, false, &mut cache).unwrap().stashes.is_empty());
    fs::write(&file, "first change\n").unwrap();
    sh(dir, &["stash", "push", "-m", "First"]);
    let first = status(dir, false, &mut cache).unwrap().stashes;
    assert_eq!(first.len(), 1);
    assert!(Arc::ptr_eq(&first, &status(dir, false, &mut cache).unwrap().stashes));
    fs::write(&file, "another change\n").unwrap();
    sh(dir, &["stash", "push", "-m", "Second"]);
    assert_eq!(status(dir, false, &mut cache).unwrap().stashes.len(), 2);
    sh(dir, &["stash", "clear"]);
    assert!(status(dir, false, &mut cache).unwrap().stashes.is_empty());
}

#[test]
fn git_cancelled_clone_removes_only_its_new_directory() {
    let temp = temp("clone cleanup");
    let clone = CloneDestination::prepare(&temp.0, "https://example.test/owner/repo.git").unwrap();
    assert_eq!(clone.path, temp.0.join("repo"));
    fs::create_dir(clone.path.join(".git")).unwrap();
    fs::write(clone.path.join(".git/index.lock"), "partial checkout").unwrap();
    assert!(CloneDestination::prepare(&temp.0, "ssh://git@other/owner/repo.git").is_err());
    clone.clean(true).unwrap();
    assert!(!clone.path.exists());
    drop(clone);

    fs::create_dir(temp.0.join("repo")).unwrap();
    let existing = CloneDestination::prepare(&temp.0, "https://example.test/owner/repo.git").unwrap();
    fs::write(existing.path.join("keep.txt"), "existing").unwrap();
    existing.clean(true).unwrap();
    assert!(existing.path.join("keep.txt").is_file());

    let owned = CloneDestination::prepare(&temp.0, "https://example.test/owner/new.git").unwrap();
    fs::rename(&owned.path, temp.0.join("moved")).unwrap();
    fs::create_dir(&owned.path).unwrap();
    owned.clean(true).unwrap();
    assert!(owned.path.is_dir());
}

#[test]
fn git_tracked_clone_keeps_completed_repositories_and_can_clone_a_local_bundle() {
    let temp = temp("tracked clone");
    let root = &temp.0;
    let source = root.join("source.git");
    sh(root, &["init", "-q", source.to_str().unwrap()]);
    sh(&source, &["commit", "--allow-empty", "-qm", "base"]);
    let bundle = root.join("backup.bundle");
    sh(&source, &["bundle", "create", bundle.to_str().unwrap(), "--all"]);
    let (events, _) = unbounded();
    let wake: Wake = Arc::new(|| {});
    for (url, name) in [(&source, "source"), (&bundle, "backup")] {
        let dir = FsPath::parse(&root.to_string_lossy()).unwrap();
        let op = GitOp::Clone { url: url.to_string_lossy().into_owned() };
        let result = execute_tracked(1, dir, op, GitJob { id: 1, ctl: OpCtl::default() }, &events, &wake);
        assert!(matches!(result, GitEvent::TrackedDone { result: GitResult::Success, .. }), "{result:?}");
        assert!(root.join(name).join(".git/HEAD").is_file());
    }
}

#[test]
fn git_index_lock_cleanup_rejects_old_and_replaced_locks() {
    let temp = temp("index lock");
    let lock = temp.0.join("index.lock");
    let touch = |time| fs::File::create(&lock).unwrap().set_times(fs::FileTimes::new().set_modified(time)).unwrap();
    let started = SystemTime::now();
    touch(started - Duration::from_secs(10));
    assert!(new_index_lock(&lock, started).is_none());
    touch(started + Duration::from_secs(10));
    let offered = new_index_lock(&lock, started).unwrap();
    assert!(lock.exists());
    let old = temp.0.join("old.lock");
    fs::rename(&lock, old).unwrap();
    touch(offered.modified);
    assert!(remove_index_lock(&offered).is_err());
    assert!(lock.exists());
    remove_index_lock(&index_lock(&lock).unwrap()).unwrap();
    assert!(!lock.exists());
}

#[test]
fn git_cancellation_allows_git_to_remove_its_index_lock() {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows::Win32::System::Threading::DETACHED_PROCESS;
        if std::env::var_os("FLIT_GIT_CANCEL_TEST_CHILD").is_none() {
            let child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "git::tests::git_cancellation_allows_git_to_remove_its_index_lock", "--nocapture"])
                .env("FLIT_GIT_CANCEL_TEST_CHILD", "1")
                .creation_flags(DETACHED_PROCESS.0)
                .output().unwrap();
            assert!(child.status.success(), "{}\n{}", String::from_utf8_lossy(&child.stdout), String::from_utf8_lossy(&child.stderr));
            assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
            return;
        }
    }
    let temp = temp("graceful git cancel");
    let dir = &temp.0;
    sh(dir, &["init", "-q"]);
    sh(dir, &["config", "filter.block.clean", "touch flit-started; sleep 20; cat"]);
    fs::write(dir.join(".gitattributes"), "slow.txt filter=block\n").unwrap();
    fs::write(dir.join("slow.txt"), "contents\n").unwrap();
    let mut cmd = command(dir);
    cmd.args(["add", "slow.txt"]);
    let ctl = OpCtl::default();
    let stop = ctl.clone();
    let worker = std::thread::spawn(move || process::run(cmd, &stop, |_| {}));
    let deadline = Instant::now() + Duration::from_secs(8);
    let marker = dir.join("flit-started");
    while !marker.exists() && !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let lock = dir.join(".git/index.lock");
    let blocking = marker.exists() && lock.exists();
    ctl.cancel();
    let result = worker.join().unwrap();
    assert!(blocking, "{result:?}");
    assert!(matches!(result, Err(process::RunError::Cancelled { .. })), "{result:?}");
    assert!(!lock.exists(), "Git did not get to remove its lock: {result:?}");
}

#[cfg(unix)]
#[test]
fn git_cancellation_escalates_if_the_group_ignores_sigterm() {
    let temp = temp("force git cancel");
    let mut cmd = Command::new("sh");
    cmd.current_dir(&temp.0).args(["-c", "trap '' TERM; touch started; exec sleep 15"]);
    let ctl = OpCtl::default();
    let stop = ctl.clone();
    let worker = std::thread::spawn(move || process::run(cmd, &stop, |_| {}));
    let marker = temp.0.join("started");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let started = Instant::now();
    ctl.cancel();
    assert_eq!(worker.join().unwrap(), Err(process::RunError::Cancelled { forced: true }));
    assert!(marker.is_file());
    assert!(started.elapsed() >= Duration::from_secs(2));
}

#[test]
fn abort_restores_each_conflicted_sequence_and_refuses_a_different_operation() {
    for (tag, kind, start) in [
        ("merge", git::Sequence::Merge, "merge"),
        ("rebase", git::Sequence::Rebase, "rebase"),
        ("pick", git::Sequence::CherryPick, "cherry-pick"),
    ] {
        let t = temp(&format!("abort {tag} папка"));
        let r = &t.0;
        sh(r, &["init", "-q"]);
        fs::write(r.join("conflict.txt"), "base\n").unwrap();
        sh(r, &["add", "."]);
        sh(r, &["commit", "-qm", "base"]);
        sh(r, &["switch", "-c", "topic/x"]);
        fs::write(r.join("conflict.txt"), "topic\n").unwrap();
        sh(r, &["commit", "-qam", "topic"]);
        sh(r, &["switch", "main"]);
        fs::write(r.join("conflict.txt"), "main\n").unwrap();
        sh(r, &["commit", "-qam", "main"]);
        let head = output(r, &["rev-parse", "HEAD"]).unwrap().stdout;
        assert!(output(r, &[start, "topic/x"]).is_err());
        let info = status(r, false, &mut Cache::default()).unwrap();
        assert_eq!(info.sequence, Some(kind));
        assert_eq!(info.conflicts, 1);
        assert_eq!(info.status.get(&b"conflict.txt"[..]), Some(&GitStatus::Conflict));
        fs::create_dir(r.join("subfolder")).unwrap();
        let nested = status(&r.join("subfolder"), false, &mut Cache::default()).unwrap();
        assert_eq!(nested.sequence, Some(kind));
        assert_eq!(nested.conflicts, 1);
        assert!(nested.status.is_empty());
        let wrong = if kind == git::Sequence::Merge { git::Sequence::Rebase } else { git::Sequence::Merge };
        assert!(run_op(r, &GitOp::Abort(wrong)).is_err());
        assert_eq!(status(r, false, &mut Cache::default()).unwrap().sequence, Some(kind));
        assert_eq!(run_op(r, &GitOp::Abort(kind)), Ok(None));
        let info = status(r, false, &mut Cache::default()).unwrap();
        assert!(info.sequence.is_none());
        assert_eq!(info.conflicts, 0);
        assert_eq!(fs::read_to_string(r.join("conflict.txt")).unwrap(), "main\n");
        assert_eq!(output(r, &["rev-parse", "HEAD"]).unwrap().stdout, head);
        assert!(run_op(r, &GitOp::Abort(kind)).is_err());
    }
}

#[test]
fn sequence_markers_belong_to_the_worktree_git_directory() {
    let t = temp("sequence worktree папка");
    let r = &t.0;
    sh(r, &["init", "-q"]);
    assert!(status(r, false, &mut Cache::default()).unwrap().sequence.is_none());
    sh(r, &["commit", "--allow-empty", "-qm", "first"]);
    sh(r, &["worktree", "add", "-b", "topic/x", "linked"]);
    let w = r.join("linked");
    let (_, gitdir) = discover(&w).unwrap();
    assert!(w.join(".git").is_file());
    for (marker, kind, directory) in [
        ("MERGE_HEAD", git::Sequence::Merge, false),
        ("rebase-merge", git::Sequence::Rebase, true),
        ("rebase-apply", git::Sequence::Rebase, true),
        ("CHERRY_PICK_HEAD", git::Sequence::CherryPick, false),
    ] {
        let p = gitdir.join(marker);
        if directory { fs::create_dir(&p).unwrap(); } else { fs::write(&p, output(r, &["rev-parse", "HEAD"]).unwrap().stdout).unwrap(); }
        assert_eq!(status(&w, false, &mut Cache::default()).unwrap().sequence, Some(kind));
        assert!(status(r, false, &mut Cache::default()).unwrap().sequence.is_none());
        if directory { fs::remove_dir(&p).unwrap(); } else { fs::remove_file(&p).unwrap(); }
    }
    assert!(status(&w, false, &mut Cache::default()).unwrap().sequence.is_none());
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> Temp {
    let d = std::env::temp_dir().join(format!("flit-git-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    Temp(d)
}

fn sh(dir: &Path, args: &[&str]) {
    let out = command(dir).args(["-c", "user.name=Ann", "-c", "user.email=ann@example.com", "-c", "init.defaultBranch=main", "-c", "core.autocrlf=false"]).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn have_git() -> bool {
    command(&std::env::temp_dir()).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[test]
fn github_login_prefers_gcm_and_uses_the_supplied_path_for_gh() {
    let t = temp("github programs");
    let path = std::env::join_paths([&t.0]).unwrap();
    assert_eq!(select_github_login(Some(&path), |_| panic!("git unavailable"), "gh"), GithubLogin::Install("gh"));
    for name in ["gh", "git"] {
        let filename = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
        let file = t.0.join(filename);
        fs::write(&file, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let gh = select_github_login(Some(&path), |_| false, "gh");
    assert!(matches!(gh, GithubLogin::Command(argv) if argv[0].contains("gh") && argv[1..] == ["auth", "login", "--web"]));
    let gcm = select_github_login(Some(&path), |_| true, "gh");
    assert!(matches!(gcm, GithubLogin::Command(argv) if argv[0].contains("git") && argv[1..] == ["credential-manager", "github", "login"]));
}

#[test]
fn github_auth_uses_the_remote_for_the_failing_command() {
    let t = temp("github remotes");
    let r = &t.0;
    sh(r, &["init", "-q"]);
    sh(r, &["commit", "--allow-empty", "-qm", "base"]);
    sh(r, &["remote", "add", "origin", "https://gitlab.com/owner/repo"]);
    sh(r, &["remote", "add", "upstream", "https://github.com/owner/repo"]);
    sh(r, &["remote", "set-url", "--push", "upstream", "git@github.com:owner/repo"]);
    sh(r, &["config", "branch.main.remote", "upstream"]);
    sh(r, &["config", "branch.main.pushRemote", "origin"]);
    assert!(git::github_remote(&auth_remote(r, &GitOp::Sync, &["pull"], "").unwrap()));
    assert!(!git::github_remote(&auth_remote(r, &GitOp::Sync, &["push"], "").unwrap()));
    sh(r, &["config", "branch.main.pushRemote", "upstream"]);
    assert_eq!(auth_remote(r, &GitOp::Push, &["push"], "").as_deref(), Some("git@github.com:owner/repo"));
    assert!(!git::github_remote(&auth_remote(r, &GitOp::Push, &["push", "--set-upstream", "origin", "HEAD"], "").unwrap()));
    sh(r, &["remote", "set-url", "--add", "--push", "upstream", "https://gitlab.com/owner/repo"]);
    let error = "fatal: Authentication failed for 'https://gitlab.com/owner/repo'";
    assert_eq!(auth_remote(r, &GitOp::Push, &["push"], error).as_deref(), Some("https://gitlab.com/owner/repo"));
    assert!(auth_remote(r, &GitOp::Push, &["push"], "Authentication failed").is_none());
    sh(r, &["remote", "remove", "origin"]);
    sh(r, &["config", "--unset", "branch.main.remote"]);
    assert!(git::github_remote(&auth_remote(r, &GitOp::Fetch, &["fetch", "--prune"], "").unwrap()));
}

#[test]
fn cancelling_git_push_stops_the_receive_hook_and_its_processes() {
    let t = temp("cancel push");
    let bare = t.0.join("remote.git");
    let work = t.0.join("work");
    sh(&t.0, &["init", "-q", "--bare", bare.to_str().unwrap()]);
    sh(&t.0, &["init", "-q", work.to_str().unwrap()]);
    sh(&work, &["commit", "--allow-empty", "-qm", "first"]);
    sh(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    let hook = bare.join("hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\nprintf started > flit-started\nsleep 4\nprintf survived > flit-survived\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut cmd = command(&work);
    cmd.args(["push", "--progress", "--set-upstream", "origin", "HEAD"]);
    let ctl = flit_core::provider::OpCtl::default();
    let stop = ctl.clone();
    let push = std::thread::spawn(move || process::run(cmd, &stop, |_| {}));
    let started = bare.join("flit-started");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !started.exists() && !push.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    ctl.cancel();
    let result = push.join().unwrap();
    assert!(started.exists(), "{result:?}");
    assert!(matches!(result, Err(process::RunError::Cancelled { .. })));
    std::thread::sleep(Duration::from_millis(4200));
    assert!(!bare.join("flit-survived").exists(), "the receive hook survived cancellation");
    assert!(output(&bare, &["rev-parse", "--verify", "refs/heads/main"]).is_err());
}

#[test]
fn switch_refusal_offers_retries_and_can_restore_the_named_stash() {
    for (tag, choice) in [("stash", SwitchChoice::Stash), ("discard", SwitchChoice::Discard), ("merge", SwitchChoice::Merge)] {
        let t = temp(tag);
        let r = &t.0;
        sh(r, &["init", "-q"]);
        let file = r.join("a.txt");
        fs::write(&file, "base\n").unwrap();
        sh(r, &["add", "."]);
        sh(r, &["commit", "-qm", "base"]);
        sh(r, &["switch", "-c", "dev"]);
        fs::write(&file, "dev\n").unwrap();
        let added = r.join("added.txt");
        fs::write(&added, "tracked on dev\n").unwrap();
        sh(r, &["add", "added.txt"]);
        sh(r, &["commit", "-qam", "dev"]);
        sh(r, &["switch", "main"]);
        fs::write(&file, "local\n").unwrap();
        let dir = FsPath::parse(&r.to_string_lossy()).unwrap();
        let ev = execute(1, dir.clone(), GitOp::Switch("dev".into()));
        assert!(matches!(ev, GitEvent::SwitchBlocked { from, branch, .. } if &from == "main" && &branch == "dev"));
        assert!(matches!(execute(1, dir.clone(), GitOp::Switch("missing".into())), GitEvent::Done { error: Some(_), .. }));
        assert_eq!(fs::read_to_string(&file).unwrap(), "local\n");
        let retry = GitOp::RetrySwitch { from: "main".into(), branch: "dev".into(), choice };
        if choice == SwitchChoice::Discard {
            fs::write(&added, "new untracked file\n").unwrap();
            assert!(run_op(r, &retry).is_err());
            assert_eq!(fs::read_to_string(&added).unwrap(), "new untracked file\n");
            fs::remove_file(&added).unwrap();
        }
        assert!(matches!(execute(1, dir.clone(), retry.clone()), GitEvent::Done { error: None, .. }));
        let info = status(r, false, &mut Cache::default()).unwrap();
        assert_eq!(info.branch, "dev");
        assert!(run_op(r, &retry).is_err(), "a stale dialog cannot act on a different branch");
        match choice {
            SwitchChoice::Merge => assert_eq!(info.status.get(&b"a.txt"[..]), Some(&GitStatus::Conflict)),
            SwitchChoice::Discard => assert_eq!(fs::read_to_string(&file).unwrap(), "dev\n"),
            SwitchChoice::Stash => {
                let stash = info.stashes.iter().find(|s| s.flit_branch() == Some("main")).unwrap();
                sh(r, &["switch", "main"]);
                fs::write(&file, "external\n").unwrap();
                sh(r, &["stash", "push", "-m", "External changes"]);
                run_op(r, &GitOp::PopStash(stash.oid.clone())).unwrap();
                assert_eq!(fs::read_to_string(&file).unwrap(), "local\n");
                let left = stashes(r);
                assert_eq!(left.len(), 1);
                assert!(left[0].0.flit_branch().is_none());
                assert!(run_op(r, &GitOp::PopStash(stash.oid.clone())).is_err());
            }
        }
    }
}

#[test]
fn local_branches_can_be_created_with_or_without_switching_and_invalid_names_change_nothing() {
    let t = temp("branches путь с пробелами");
    let r = &t.0;
    sh(r, &["init", "-q"]);
    assert!(run_op(r, &GitOp::CreateBranch { name: "empty".into(), base: "HEAD".into(), switch: false }).is_err());
    sh(r, &["commit", "--allow-empty", "-qm", "first"]);
    let before = branches(r);
    for name in ["", "bad name", "bad..name", "-bad", "topic/", "foo.lock"] {
        assert!(run_op(r, &GitOp::CreateBranch { name: name.into(), base: "main".into(), switch: true }).is_err(), "{name}");
        assert_eq!(branches(r), before);
        assert_eq!(status(r, false, &mut Cache::default()).unwrap().branch, "main");
    }
    run_op(r, &GitOp::CreateBranch { name: "origin/topic".into(), base: "main".into(), switch: false }).unwrap();
    assert_eq!(status(r, false, &mut Cache::default()).unwrap().branch, "main");
    assert!(branches(r).iter().any(|b| b.name == "origin/topic" && !b.remote));
    sh(r, &["branch", "feature/HEAD"]);
    assert!(branches(r).iter().any(|b| b.name == "feature/HEAD" && !b.remote));
    run_op(r, &GitOp::CreateBranch { name: "topic/новая".into(), base: "origin/topic".into(), switch: true }).unwrap();
    assert_eq!(status(r, false, &mut Cache::default()).unwrap().branch, "topic/новая");
    assert!(run_op(r, &GitOp::CreateBranch { name: "origin/topic".into(), base: "main".into(), switch: false }).is_err());
    sh(r, &["switch", "--detach"]);
    run_op(r, &GitOp::CreateBranch { name: "from-detached".into(), base: "HEAD".into(), switch: true }).unwrap();
    assert_eq!(status(r, false, &mut Cache::default()).unwrap().branch, "from-detached");
}

#[test]
fn deleting_a_branch_preserves_unmerged_commits_until_force_is_requested() {
    let t = temp("delete-branch");
    let r = &t.0;
    sh(r, &["init", "-q"]);
    sh(r, &["commit", "--allow-empty", "-qm", "first"]);
    sh(r, &["branch", "merged"]);
    run_op(r, &GitOp::DeleteBranch { name: "merged".into(), force: false }).unwrap();
    sh(r, &["switch", "-c", "topic/x"]);
    sh(r, &["commit", "--allow-empty", "-qm", "unmerged"]);
    let tip = output(r, &["rev-parse", "HEAD"]).unwrap().stdout;
    sh(r, &["switch", "main"]);
    let error = run_op(r, &GitOp::DeleteBranch { name: "topic/x".into(), force: false }).unwrap_err();
    assert!(error.contains("not fully merged"), "{error}");
    assert_eq!(output(r, &["rev-parse", "topic/x"]).unwrap().stdout, tip);
    run_op(r, &GitOp::DeleteBranch { name: "topic/x".into(), force: true }).unwrap();
    assert!(!branches(r).iter().any(|b| b.name == "topic/x"));
    assert!(run_op(r, &GitOp::DeleteBranch { name: "main".into(), force: true }).is_err());
    assert_eq!(status(r, false, &mut Cache::default()).unwrap().branch, "main");
}

#[test]
fn status_branch_and_last_commits_of_a_folder() {
    if !have_git() {
        return;
    }
    let t = temp("status");
    let r = &t.0;
    fs::create_dir_all(r.join("src/deep")).unwrap();
    fs::write(r.join("src/deep/x.txt"), "1").unwrap();
    fs::write(r.join("src/a.txt"), "1").unwrap();
    fs::write(r.join("top.txt"), "1").unwrap();
    sh(r, &["init", "-q"]);
    sh(r, &["add", "."]);
    sh(r, &["commit", "-qm", "first"]);
    fs::write(r.join("src/a.txt"), "2").unwrap();
    sh(r, &["commit", "-qam", "second"]);
    fs::write(r.join("src/deep/x.txt"), "3").unwrap();
    fs::write(r.join("src/new.txt"), "n").unwrap();
    let src = std_path(&FsPath::parse(&r.join("src").to_string_lossy()).unwrap());
    let mut cache = Cache::default();
    let info = status(&src, true, &mut cache).unwrap();
    assert_eq!(info.branch, "main");
    assert_eq!(info.status.get(&b"deep"[..]), Some(&GitStatus::Modified));
    assert_eq!(info.status.get(&b"new.txt"[..]), Some(&GitStatus::Untracked));
    assert_eq!(info.status.get(&b"a.txt"[..]), None);
    assert_eq!(info.commits[&b"a.txt"[..]].message, "second");
    assert_eq!(info.commits[&b"deep"[..]].message, "first");
    assert_eq!(info.commits[&b"a.txt"[..]].author, "Ann");
    let commit = &info.commits[&b"a.txt"[..]];
    assert_eq!(commit.short_hash.len(), 7);
    assert_eq!(commit.short_hash, commit.hash[..7]);
    assert_eq!(commit.hash.len(), 40);
    let again = status(&src, true, &mut cache).unwrap();
    assert!(Arc::ptr_eq(&again.commits[&b"a.txt"[..]], &info.commits[&b"a.txt"[..]]), "the log is cached while HEAD stays");
    let outside = temp("plain");
    assert!(discover(&outside.0).is_none_or(|(root, _)| !root.starts_with(&outside.0)));
}

#[test]
fn network_operations_against_a_local_bare_repository() {
    if !have_git() {
        return;
    }
    let t = temp("net");
    let (bare, work) = (t.0.join("origin.git"), t.0.join("work"));
    fs::create_dir_all(&bare).unwrap();
    sh(&bare, &["init", "-q", "--bare"]);
    fs::create_dir_all(&work).unwrap();
    run_op(&work, &GitOp::Init).unwrap();
    assert!(discover(&work).is_some_and(|(root, _)| root == work));
    sh(&work, &["checkout", "-q", "-b", "main"]);
    fs::write(work.join("a.txt"), "1").unwrap();
    sh(&work, &["add", "."]);
    sh(&work, &["commit", "-qm", "one"]);
    sh(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    let no_upstream = output(&work, &["push"]).unwrap_err();
    assert!(NO_UPSTREAM.iter().any(|s| no_upstream.contains(s)), "{no_upstream}");
    run_op(&work, &GitOp::Push).unwrap();
    let url = bare.to_string_lossy().into_owned();
    run_op(&t.0, &GitOp::Clone { url }).unwrap();
    let clone = t.0.join("origin");
    assert_eq!(fs::read_to_string(clone.join("a.txt")).unwrap(), "1");
    fs::write(work.join("a.txt"), "2").unwrap();
    sh(&work, &["commit", "-qam", "two"]);
    run_op(&work, &GitOp::Push).unwrap();
    run_op(&clone, &GitOp::Fetch).unwrap();
    run_op(&clone, &GitOp::Pull).unwrap();
    assert_eq!(fs::read_to_string(clone.join("a.txt")).unwrap(), "2");
    fs::write(work.join("a.txt"), "3").unwrap();
    sh(&work, &["commit", "-qam", "three"]);
    run_op(&work, &GitOp::Push).unwrap();
    run_op(&clone, &GitOp::Sync).unwrap();
    assert_eq!(fs::read_to_string(clone.join("a.txt")).unwrap(), "3");
    sh(&work, &["branch", "dev"]);
    run_op(&work, &GitOp::Push).unwrap();
    sh(&work, &["push", "-q", "origin", "dev"]);
    run_op(&clone, &GitOp::Fetch).unwrap();
    let list = branches(&clone);
    assert!(list.iter().any(|b| b.name == "main" && !b.remote) && list.iter().any(|b| b.name == "origin/dev" && b.remote), "{list:?}");
    assert!(!list.iter().any(|b| b.name.ends_with("HEAD")));
    run_op(&clone, &GitOp::Switch("origin/dev".into())).unwrap();
    assert_eq!(status(&clone, false, &mut Cache::default()).unwrap().branch, "dev");
    assert!(run_op(&clone, &GitOp::Switch("missing".into())).is_err());
    assert!(run_op(&clone, &GitOp::OpenRemote).is_err(), "a path remote has no web page");
    sh(&clone, &["remote", "set-url", "origin", "git@github.com:o/r.git"]);
    assert_eq!(run_op(&clone, &GitOp::OpenRemote).unwrap().as_deref(), Some("https://github.com/o/r"));
}

#[test]
#[ignore]
fn bench_status_of_a_large_folder() {
    let Some(dir) = std::env::var_os("FLIT_GIT_BENCH").map(PathBuf::from) else { return };
    let mut cache = Cache::default();
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let plain = status(&dir, false, &mut cache).unwrap();
    let status_ms = ms(t);
    let t = Instant::now();
    let full = status(&dir, true, &mut cache).unwrap();
    let cold_ms = ms(t);
    let t = Instant::now();
    status(&dir, true, &mut cache).unwrap();
    let warm_ms = ms(t);
    let t = Instant::now();
    assert!(discover(&std::env::temp_dir()).is_none_or(|(r, _)| r != std::env::temp_dir()));
    let miss_us = t.elapsed().as_secs_f64() * 1e6;
    eprintln!("{}: {} statuses in {status_ms:.1} ms; {} last commits in {cold_ms:.1} ms cold, {warm_ms:.1} ms cached; outside a repository {miss_us:.0} us", dir.display(), plain.status.len(), full.commits.len());
}

#[test]
fn a_git_that_cannot_be_started_is_reported_as_missing() {
    assert_eq!(spawn_error(&std::io::Error::from(std::io::ErrorKind::NotFound)), S::GitErrorMissing.en());
    assert_eq!(flit_core::git::failure(&spawn_error(&std::io::Error::from(std::io::ErrorKind::NotFound))), flit_core::git::Failure::Missing);
    let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert_eq!(spawn_error(&denied), denied.to_string(), "other start failures keep their own reason");
}
