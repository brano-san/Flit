use super::*;
use flit_core::git::GitStatus;
use std::fs;

struct Temp(PathBuf);

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
    sh(&work, &["branch", "dev"]);
    run_op(&work, &GitOp::Push).unwrap();
    sh(&work, &["push", "-q", "origin", "dev"]);
    run_op(&clone, &GitOp::Fetch).unwrap();
    let list = branches(&clone);
    assert!(list.contains(&"main".to_owned()) && list.contains(&"origin/dev".to_owned()), "{list:?}");
    assert!(!list.iter().any(|b| b.ends_with("HEAD")));
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
