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

struct Temp(PathBuf);

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
    Command::new("git").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
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
