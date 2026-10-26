use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::git::{self, Commit, GitEvent, GitInfo, GitOp, GitRequest, LogParser};
use flit_core::hash::FxHashMap;
use flit_core::i18n::S;
use flit_core::FsPath;
use flit_platform::{PlatformError, Wake};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const SETTLE: Duration = Duration::from_millis(150);
const LOG_BUDGET: Duration = Duration::from_secs(3);
const COMMIT_CACHE: usize = 64;

type Commits = FxHashMap<Vec<u8>, Arc<Commit>>;
type HeadKey = (PathBuf, Vec<u8>, SystemTime, u64);
type Job = GitRequest;
type Reply = GitEvent;

pub struct GitWorker {
    jobs: Sender<Job>,
    pub events: Receiver<Reply>,
}

impl GitWorker {
    pub fn start(wake: Wake) -> Result<Self, PlatformError> {
        let (jobs, rx) = unbounded::<Job>();
        let (tx, events) = unbounded();
        std::thread::Builder::new()
            .name("flit-git".into())
            .spawn(move || run(rx, tx, wake))
            .map_err(|e| PlatformError::new(-1, format!("cannot start flit-git: {e}")))?;
        Ok(Self { jobs, events })
    }

    pub fn submit(&self, req: GitRequest) {
        let _ = self.jobs.send(req);
    }
}

pub fn command(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("--no-optional-locks").arg("-C").arg(dir);
    c.env("GIT_TERMINAL_PROMPT", "0").env("GIT_MERGE_AUTOEDIT", "no").env("LC_ALL", "C");
    c.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn output(dir: &Path, args: &[&str]) -> Result<Output, String> {
    let out = command(dir).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(out);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let mut lines = err.lines().map(str::trim).filter(|l| !l.is_empty());
    let first_error = lines.clone().find(|l| l.starts_with("fatal:") || l.starts_with("error:"));
    Err(first_error.or_else(|| lines.next_back()).unwrap_or("git failed").to_owned())
}

pub fn discover(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let dot = d.join(".git");
        if dot.is_dir() {
            return Some((d.to_path_buf(), dot));
        }
        if let Ok(text) = std::fs::read(&dot) {
            let link = git::gitdir_link(&text)?;
            let link = PathBuf::from(String::from_utf8_lossy(link).into_owned());
            return Some((d.to_path_buf(), d.join(link)));
        }
        cur = d.parent();
    }
    None
}

fn prefix(root: &Path, dir: &Path) -> Vec<u8> {
    let Ok(rel) = dir.strip_prefix(root) else { return Vec::new() };
    let mut p = Vec::new();
    for c in rel.components() {
        p.extend_from_slice(c.as_os_str().to_string_lossy().as_bytes());
        p.push(b'/');
    }
    p
}

fn children(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |r| r.filter_map(Result::ok).filter(|e| e.file_name() != ".git").count())
}

pub fn last_commits(dir: &Path) -> Commits {
    let mut commits = Commits::default();
    let want = children(dir);
    let Ok(mut child) = command(dir).args(["-c", "core.quotePath=false", "log", LogParser::FORMAT, "--name-only", "-z", "--relative", "--", "."]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else {
        return commits;
    };
    let started = Instant::now();
    if let Some(out) = child.stdout.take() {
        let mut parser = LogParser::new();
        for field in BufReader::new(out).split(0).map_while(Result::ok) {
            parser.feed(&field, &mut commits);
            if commits.len() >= want || started.elapsed() > LOG_BUDGET {
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    commits
}

fn head_stamp(gitdir: &Path) -> Option<(Vec<u8>, SystemTime, u64)> {
    let head = std::fs::read(gitdir.join("HEAD")).ok()?;
    let common = std::fs::read(gitdir.join("commondir")).ok().map(|c| gitdir.join(String::from_utf8_lossy(c.trim_ascii()).into_owned())).unwrap_or_else(|| gitdir.to_path_buf());
    let log = std::fs::metadata(gitdir.join("logs").join("HEAD")).or_else(|_| std::fs::metadata(common.join("packed-refs"))).ok()?;
    Some((head, log.modified().ok()?, log.len()))
}

#[derive(Default)]
pub struct Cache {
    commits: Vec<(HeadKey, Commits)>,
}

impl Cache {
    fn commits(&mut self, dir: &Path, gitdir: &Path) -> Commits {
        let Some((head, time, len)) = head_stamp(gitdir) else { return last_commits(dir) };
        let key = (dir.to_path_buf(), head, time, len);
        if let Some((_, c)) = self.commits.iter().find(|(k, _)| *k == key) {
            return c.clone();
        }
        let c = last_commits(dir);
        if self.commits.len() >= COMMIT_CACHE {
            self.commits.remove(0);
        }
        self.commits.push((key, c.clone()));
        c
    }
}

pub fn status(dir: &Path, commits: bool, cache: &mut Cache) -> Option<GitInfo> {
    let (root, gitdir) = discover(dir)?;
    let branch = std::fs::read(gitdir.join("HEAD")).ok().and_then(|h| git::head_branch(&h)).unwrap_or_default();
    let status = output(dir, &["status", "--porcelain=v1", "-z", "--", "."]).map(|o| git::parse_status(&o.stdout, &prefix(&root, dir))).unwrap_or_default();
    let commits = if commits { cache.commits(dir, &gitdir) } else { Commits::default() };
    Some(GitInfo { root: FsPath::parse(&root.to_string_lossy()), branch, status, commits })
}


pub fn branches(dir: &Path) -> Vec<String> {
    let Ok(out) = output(dir, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes"]) else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.ends_with("/HEAD"))
        .filter_map(|l| l.strip_prefix("refs/heads/").or_else(|| l.strip_prefix("refs/remotes/")))
        .map(str::to_owned)
        .collect()
}

fn remote_url(dir: &Path) -> Result<String, String> {
    let out = output(dir, &["remote"])?;
    let names = String::from_utf8_lossy(&out.stdout).into_owned();
    let name = names.lines().find(|n| *n == "origin").or_else(|| names.lines().next()).ok_or_else(|| S::GitErrorNoWeb.t().to_owned())?;
    let out = output(dir, &["remote", "get-url", name])?;
    git::web_url(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| S::GitErrorNoWeb.t().to_owned())
}

pub fn run_op(dir: &Path, op: &GitOp) -> Result<Option<String>, String> {
    match op {
        GitOp::Fetch => output(dir, &["fetch", "--prune"]).map(drop),
        GitOp::Pull => output(dir, &["pull"]).map(drop),
        GitOp::Push => match output(dir, &["push"]) {
            Err(e) if e.contains("no upstream") || e.contains("set-upstream") => output(dir, &["push", "--set-upstream", "origin", "HEAD"]).map(drop),
            r => r.map(drop),
        },
        GitOp::Init => output(dir, &["init"]).map(drop),
        GitOp::Clone { url } => output(dir, &["clone", "--", url]).map(drop),
        GitOp::Switch(b) => match output(dir, &["switch", b]) {
            Err(e) => match b.split_once('/') {
                Some((_, tail)) if !tail.is_empty() => output(dir, &["switch", tail]).map(drop).map_err(|_| e),
                _ => Err(e),
            },
            ok => ok.map(drop),
        },
        GitOp::OpenRemote => return remote_url(dir).map(Some),
    }
    .map(|()| None)
}

#[cfg(windows)]
pub fn std_path(p: &FsPath) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let mut w = p.to_wide(false);
    w.pop();
    PathBuf::from(std::ffi::OsString::from_wide(&w))
}

#[cfg(unix)]
pub fn std_path(p: &FsPath) -> PathBuf {
    crate::preview::std_path(p)
}

fn run(rx: Receiver<Job>, tx: Sender<Reply>, wake: Wake) {
    let mut cache = Cache::default();
    while let Ok(first) = rx.recv() {
        if matches!(first, GitRequest::Status { changed: true, .. }) {
            std::thread::sleep(SETTLE);
        }
        let mut batch = vec![first];
        batch.extend(rx.try_iter());
        let latest = |k: usize, b: &[Job]| match &b[k] {
            GitRequest::Status { tab, .. } => !b[k + 1..].iter().any(|r| matches!(r, GitRequest::Status { tab: t, .. } if t == tab)),
            _ => true,
        };
        let keep: Vec<bool> = (0..batch.len()).map(|k| latest(k, &batch)).collect();
        for (req, keep) in batch.into_iter().zip(keep) {
            if !keep {
                continue;
            }
            let reply = match req {
                GitRequest::Status { tab, gen, dir, commits, .. } => {
                    let started = Instant::now();
                    let info = status(&std_path(&dir), commits, &mut cache);
                    tracing::debug!("git status of {} in {:.1} ms, repository: {}", dir.display(), started.elapsed().as_secs_f64() * 1e3, info.is_some());
                    GitEvent::Status { tab, gen, info: info.map(Arc::new) }
                }
                GitRequest::Branches { tab, dir } => GitEvent::Branches { tab, branches: branches(&std_path(&dir)) },
                GitRequest::Run { tab, dir, op } => {
                    let (tx, wake) = (tx.clone(), wake.clone());
                    let _ = tx.send(GitEvent::Started { tab, op: op.clone() });
                    wake();
                    let spawned = std::thread::Builder::new().name("flit-git-op".into()).spawn(move || {
                        let (error, url) = match run_op(&std_path(&dir), &op) {
                            Ok(url) => (None, url),
                            Err(e) => (Some(e), None),
                        };
                        if tx.send(GitEvent::Done { tab, op, error, url }).is_ok() {
                            wake();
                        }
                    });
                    if let Err(e) = spawned {
                        tracing::warn!("cannot start flit-git-op: {e}");
                    }
                    continue;
                }
            };
            if tx.send(reply).is_err() {
                return;
            }
            wake();
        }
    }
}

#[cfg(test)]
#[path = "../test/git.rs"]
mod tests;
