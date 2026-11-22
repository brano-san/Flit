use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::git::{self, Commit, GitEvent, GitIndexLock, GitInfo, GitJob, GitOp, GitRequest, GitResult, GithubLogin, LogParser, Stash, SwitchChoice};
use flit_core::hash::FxHashMap;
use flit_core::i18n::S;
use flit_core::provider::{OpCtl, TabId};
use flit_core::FsPath;
use flit_platform::{PlatformError, Wake};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

const SETTLE: Duration = Duration::from_millis(150);
const LOG_BUDGET: Duration = Duration::from_secs(3);
const COMMIT_CACHE: usize = 64;
static BRANCH_OP: Mutex<()> = Mutex::new(());
static CLONES: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

#[path = "git_process.rs"]
mod process;

type Commits = FxHashMap<Vec<u8>, Arc<Commit>>;
type HeadKey = (PathBuf, Vec<u8>, SystemTime, u64);
type Job = GitRequest;
type Reply = GitEvent;

pub struct GitWorker {
    jobs: Sender<Job>,
    pub events: Receiver<Reply>,
    transfers: Arc<Mutex<Transfers>>,
}

#[derive(Default)]
struct Transfers {
    closed: bool,
    active: Vec<(OpCtl, JoinHandle<()>)>,
}

impl GitWorker {
    pub fn start(wake: Wake) -> Result<Self, PlatformError> {
        let (jobs, rx) = unbounded::<Job>();
        let (tx, events) = unbounded();
        let transfers = Arc::new(Mutex::new(Transfers::default()));
        let active = transfers.clone();
        std::thread::Builder::new()
            .name("flit-git".into())
            .spawn(move || run(rx, tx, wake, &active))
            .map_err(|e| PlatformError::new(-1, format!("cannot start flit-git: {e}")))?;
        Ok(Self { jobs, events, transfers })
    }

    pub fn submit(&self, req: GitRequest) {
        let _ = self.jobs.send(req);
    }
}

impl Drop for GitWorker {
    fn drop(&mut self) {
        let active = {
            let mut transfers = self.transfers.lock().unwrap_or_else(|e| e.into_inner());
            transfers.closed = true;
            std::mem::take(&mut transfers.active)
        };
        for (ctl, _) in &active {
            ctl.cancel();
        }
        for (_, worker) in active {
            let _ = worker.join();
        }
    }
}

pub fn command(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("--no-optional-locks").arg("-C").arg(dir);
    c.env("GIT_TERMINAL_PROMPT", "0").env("GIT_MERGE_AUTOEDIT", "no").env("LC_ALL", "C").env("LANGUAGE", "");
    c.stdin(Stdio::null());
    #[cfg(test)]
    tests::isolate(&mut c);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn output(dir: &Path, args: &[&str]) -> Result<Output, String> {
    let out = command(dir).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| spawn_error(&e))?;
    if out.status.success() {
        return Ok(out);
    }
    Err(output_error(&String::from_utf8_lossy(&out.stderr)))
}

fn output_error(stderr: &str) -> String {
    let mut lines = stderr.lines().map(str::trim).filter(|l| !l.is_empty());
    let auth = lines.clone().filter(|l| git::failure(l) == git::Failure::Auth).max_by_key(|l| auth_error_host(l).is_some());
    let first_error = lines.clone().find(|l| l.starts_with("fatal:") || l.starts_with("error:"));
    auth.or(first_error).or_else(|| lines.next_back()).unwrap_or_else(|| S::GitErrorUnknown.t()).to_owned()
}

fn spawn_error(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound { S::GitErrorMissing.en().to_owned() } else { e.to_string() }
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
    let Ok(mut child) = command(dir).args(["-c", "core.quotePath=false", "log", LogParser::FORMAT, "--abbrev=7", "--name-only", "-z", "--relative", "--", "."]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else {
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
    let common = common_gitdir(gitdir);
    let log = std::fs::metadata(gitdir.join("logs").join("HEAD")).or_else(|_| std::fs::metadata(common.join("packed-refs"))).ok()?;
    Some((head, log.modified().ok()?, log.len()))
}

fn common_gitdir(gitdir: &Path) -> PathBuf {
    std::fs::read(gitdir.join("commondir")).ok().map(|c| gitdir.join(String::from_utf8_lossy(c.trim_ascii()).as_ref())).unwrap_or_else(|| gitdir.to_path_buf())
}

struct StashCache {
    modified: SystemTime,
    len: u64,
    stashes: Arc<[Stash]>,
}

#[derive(Default)]
pub struct Cache {
    commits: Vec<(HeadKey, Commits)>,
    stashes: FxHashMap<PathBuf, StashCache>,
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

    fn stashes(&mut self, root: &Path, gitdir: &Path) -> Arc<[Stash]> {
        let log = common_gitdir(gitdir).join("logs/refs/stash");
        let Some(meta) = log.metadata().ok().filter(|m| m.is_file()) else {
            self.stashes.remove(&log);
            return Arc::default();
        };
        let modified = meta.modified().ok();
        if let Some(hit) = self.stashes.get(&log).filter(|hit| Some(hit.modified) == modified && hit.len == meta.len()) {
            return hit.stashes.clone();
        }
        let Ok(entries) = read_stashes(root) else { return Arc::default() };
        let stashes: Arc<[Stash]> = entries.into_iter().map(|(stash, _)| stash).collect();
        if let Some(modified) = modified {
            if self.stashes.len() >= COMMIT_CACHE && !self.stashes.contains_key(&log) {
                self.stashes.clear();
            }
            self.stashes.insert(log, StashCache { modified, len: meta.len(), stashes: stashes.clone() });
        }
        stashes
    }
}

fn sequence(gitdir: &Path) -> Option<git::Sequence> {
    if gitdir.join("rebase-merge").is_dir() || gitdir.join("rebase-apply").is_dir() { Some(git::Sequence::Rebase) }
    else if gitdir.join("MERGE_HEAD").is_file() { Some(git::Sequence::Merge) }
    else if gitdir.join("CHERRY_PICK_HEAD").is_file() { Some(git::Sequence::CherryPick) }
    else { None }
}

pub fn status(dir: &Path, commits: bool, cache: &mut Cache) -> Option<GitInfo> {
    let (root, gitdir) = discover(dir)?;
    let branch = std::fs::read(gitdir.join("HEAD")).ok().and_then(|h| git::head_branch(&h)).unwrap_or_default();
    let args = ["status", "--porcelain=v1", "--branch", "--ahead-behind", "-z", "--", "."];
    let porcelain = output(&root, &args).map(|o| o.stdout).unwrap_or_default();
    let prefix = prefix(&root, dir);
    let scoped = (!prefix.is_empty() && git::status_entries(&porcelain).any(|(x, _, path)| x == b'?' && path.ends_with(b"/") && prefix.starts_with(path)))
        .then(|| output(dir, &args).map(|o| o.stdout).unwrap_or_default());
    let status = git::parse_status(scoped.as_deref().unwrap_or(&porcelain), &prefix);
    let commits = if commits { cache.commits(dir, &gitdir) } else { Commits::default() };
    let (upstream, ahead, behind) = git::parse_tracking(&porcelain);
    let (insertions, deletions) = if git::status_entries(&porcelain).any(|(x, _, _)| !matches!(x, b'?' | b'!')) {
        let mut args = vec!["diff", "--shortstat", "--no-ext-diff", "--no-textconv"];
        if !porcelain.starts_with(b"## No commits yet on ") && !porcelain.starts_with(b"## Initial commit on ") {
            args.extend(["HEAD", "--"]);
        }
        output(&root, &args).map(|o| git::parse_shortstat(&o.stdout)).unwrap_or_default()
    } else { (0, 0) };
    let sequence = sequence(&gitdir);
    let conflicts = if sequence.is_some() {
        output(&root, &["diff", "--name-only", "--diff-filter=U", "-z"]).map(|o| o.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()).count()).unwrap_or(0)
    } else { 0 };
    let stashes = cache.stashes(&root, &gitdir);
    Some(GitInfo { root: FsPath::parse(&root.to_string_lossy()), branch, upstream, ahead, behind, insertions, deletions, status, commits, sequence, conflicts, stashes })
}

fn stashes(dir: &Path) -> Vec<(Stash, String)> {
    read_stashes(dir).unwrap_or_default()
}

fn read_stashes(dir: &Path) -> Result<Vec<(Stash, String)>, String> {
    let out = output(dir, &["stash", "list", "-z", "--format=%H%x00%gd%x00%gs"])?;
    let fields: Vec<_> = out.stdout.split(|b| *b == 0).collect();
    Ok(fields.chunks_exact(3).map(|f| {
        let text = |k: usize| String::from_utf8_lossy(f[k]).into_owned();
        (Stash { oid: text(0), message: text(2) }, text(1))
    }).collect())
}

fn stash_selector(dir: &Path, oid: &str) -> Result<String, String> {
    stashes(dir).into_iter().find_map(|(stash, selector)| (stash.oid == oid).then_some(selector)).ok_or_else(|| S::GitStashMissing.t().to_owned())
}

struct SwitchError {
    message: String,
    blocked: bool,
}

fn switch_once(dir: &Path, branch: &str, option: Option<&str>) -> Result<(), SwitchError> {
    let out = command(dir).arg("switch").args(option).args(["--", branch]).output()
        .map_err(|e| SwitchError { message: spawn_error(&e), blocked: false })?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let blocked = out.status.code() == Some(1)
        && stderr.lines().any(|line| line.contains("Your local changes to the following files would be overwritten"))
        && !stderr.contains("untracked working tree files would be overwritten");
    Err(SwitchError { message: output_error(&stderr), blocked })
}

fn switch_branch<'a>(dir: &Path, branch: &'a str) -> Result<(), (&'a str, SwitchError)> {
    match switch_once(dir, branch, None) {
        Err(e) if e.message.starts_with("fatal: invalid reference:") || e.message.starts_with("fatal: a branch is expected, got remote branch") => {
            if let Some((_, tail)) = branch.split_once('/') {
                match switch_once(dir, tail, None) {
                    Ok(()) => return Ok(()),
                    Err(fallback) if fallback.blocked => return Err((tail, fallback)),
                    Err(_) => {}
                }
            }
            Err((branch, e))
        }
        result => result.map_err(|e| (branch, e)),
    }
}

fn current_branch(dir: &Path) -> Option<String> {
    let (_, gitdir) = discover(dir)?;
    git::head_branch(&std::fs::read(gitdir.join("HEAD")).ok()?)
}

fn retry_switch(dir: &Path, from: &str, branch: &str, choice: SwitchChoice) -> Result<bool, String> {
    if current_branch(dir).as_deref() != Some(from) {
        return Err(S::GitBranchSwitchChanged.t().to_owned());
    }
    let option = match choice {
        SwitchChoice::Merge => Some("--merge"),
        SwitchChoice::Discard => Some("--discard-changes"),
        SwitchChoice::Stash => None,
    };
    let saved = if choice == SwitchChoice::Stash {
        let out = output(dir, &["stash", "push", "-m", &format!("Flit: {from}")])?;
        !String::from_utf8_lossy(&out.stdout).contains("No local changes to save")
    } else {
        false
    };
    switch_once(dir, branch, option).map_err(|e| if saved {
        flit_core::tr!(S::GitStashSwitchFailed, error = e.message)
    } else {
        e.message
    })?;
    Ok(saved)
}

pub fn branches(dir: &Path) -> Vec<git::Branch> {
    let Ok(out) = output(dir, &["for-each-ref", "--format=%(refname)%00%(symref)", "refs/heads", "refs/remotes"]) else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once('\0').filter(|(_, target)| target.is_empty()).map(|(name, _)| name))
        .filter_map(|l| l.strip_prefix("refs/heads/").map(|name| git::Branch { name: name.into(), remote: false }).or_else(|| l.strip_prefix("refs/remotes/").map(|name| git::Branch { name: name.into(), remote: true })))
        .collect()
}

fn remote_url(dir: &Path) -> Result<String, String> {
    let out = output(dir, &["remote"])?;
    let names = String::from_utf8_lossy(&out.stdout).into_owned();
    let name = names.lines().find(|n| *n == "origin").or_else(|| names.lines().next()).ok_or_else(|| S::GitErrorNoWeb.t().to_owned())?;
    let out = output(dir, &["remote", "get-url", name])?;
    git::web_url(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| S::GitErrorNoWeb.t().to_owned())
}

const NO_UPSTREAM: [&str; 2] = ["no upstream", "set-upstream"];

fn auth_error_host(error: &str) -> Option<&str> {
    if let Some((address, _)) = error.split_once(": Permission denied") {
        return address.rsplit('@').next().filter(|host| !host.starts_with("remote: "));
    }
    if !error.starts_with("fatal:") { return None; }
    error.split('\'').nth(1).filter(|address| address.contains("://")).and_then(git::remote_host)
}

fn auth_remote(dir: &Path, op: &GitOp, args: &[&str], error: &str) -> Option<String> {
    if let GitOp::Clone { url } = op { return Some(url.clone()); }
    let config = |key: &str| output(dir, &["config", "--get", key]).ok().map(|o| String::from_utf8_lossy(o.stdout.trim_ascii()).into_owned()).filter(|s| !s.is_empty());
    let branch = current_branch(dir);
    let pushed = args.first() == Some(&"push");
    let name = if pushed && args.get(1) == Some(&"--set-upstream") {
        "origin".into()
    } else {
        let pushed = pushed.then(|| {
            branch.as_ref().and_then(|b| config(&format!("branch.{b}.pushRemote"))).or_else(|| config("remote.pushDefault"))
        }).flatten();
        pushed.or_else(|| branch.as_ref().and_then(|b| config(&format!("branch.{b}.remote")))).unwrap_or_else(|| default_remote(dir))
    };
    let mut args = vec!["remote", "get-url"];
    if pushed { args.extend(["--push", "--all"]); }
    args.extend(["--", &name]);
    let out = output(dir, &args).ok()?;
    let urls = String::from_utf8_lossy(out.stdout.trim_ascii());
    let mut lines = urls.lines();
    let first = lines.next()?;
    if lines.next().is_none() { return Some(first.to_owned()); }
    let host = auth_error_host(error)?;
    let matching = urls.lines().find(|url| git::remote_host(url).is_some_and(|h| h.eq_ignore_ascii_case(host)))?;
    Some(matching.to_owned())
}

fn default_remote(dir: &Path) -> String {
    let names = output(dir, &["remote"]).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut lines = names.lines();
    match (lines.next(), lines.next()) {
        (Some(only), None) => only.to_owned(),
        _ => "origin".into(),
    }
}

fn executable_on_path(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    #[cfg(windows)]
    let program = format!("{program}.exe");
    #[cfg(unix)]
    let program = program.to_owned();
    std::env::split_paths(path?).filter(|d| d.is_absolute()).map(|d| d.join(&program)).find(|p| {
        let Ok(meta) = p.metadata() else { return false };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 == 0 { return false; }
        }
        meta.is_file()
    })
}

fn select_github_login(path: Option<&OsStr>, gcm: impl FnOnce(&Path) -> bool, package: &'static str) -> GithubLogin {
    let command = |program: PathBuf, args: &[&str]| GithubLogin::Command(
        std::iter::once(program.to_string_lossy().into_owned()).chain(args.iter().map(|s| (*s).into())).collect()
    );
    if let Some(git) = executable_on_path("git", path).filter(|p| gcm(p)) {
        command(git, &["credential-manager", "github", "login"])
    } else if let Some(gh) = executable_on_path("gh", path) {
        command(gh, &["auth", "login", "--web"])
    } else {
        GithubLogin::Install(package)
    }
}

fn github_login(dir: &Path) -> GithubLogin {
    #[cfg(windows)]
    let package = "GitHub CLI";
    #[cfg(unix)]
    let package = flit_platform_linux_gui::shell::github_cli_package();
    select_github_login(std::env::var_os("PATH").as_deref(), |program| {
        let mut cmd = Command::new(program);
        cmd.current_dir(dir).args(["credential-manager", "--version"]).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.output().is_ok_and(|out| out.status.success())
    }, package)
}

fn auth_event(tab: flit_core::provider::TabId, dir: &FsPath, op: &GitOp, args: &[&str], error: &str) -> Option<GitEvent> {
    if git::failure(error) != git::Failure::Auth || git::op_command(op).is_none() { return None; }
    let path = std_path(dir);
    let github = auth_remote(&path, op, args, error).filter(|r| git::github_remote(r)).map(|_| github_login(&path));
    let dir = if matches!(op, GitOp::Clone { .. }) { None } else { discover(&path).and_then(|(root, _)| FsPath::parse(&root.to_string_lossy())) }.unwrap_or_else(|| dir.clone());
    Some(GitEvent::AuthFailed { tab, dir, op: op.clone(), error: error.to_owned(), github })
}

fn push(run: &mut impl FnMut(&[&str]) -> Result<(), String>) -> Result<(), String> {
    match run(&["push"]) {
        Err(e) if NO_UPSTREAM.iter().any(|s| e.contains(s)) => run(&["push", "--set-upstream", "origin", "HEAD"]),
        r => r,
    }
}

fn sync(run: &mut impl FnMut(&[&str]) -> Result<(), String>) -> Result<(), String> {
    run(&["pull"])?;
    push(run)
}

pub fn run_op(dir: &Path, op: &GitOp) -> Result<Option<String>, String> {
    run_op_with(dir, op, &mut |args| output(dir, args).map(drop))
}

fn run_op_with(dir: &Path, op: &GitOp, network: &mut impl FnMut(&[&str]) -> Result<(), String>) -> Result<Option<String>, String> {
    match op {
        GitOp::Fetch => network(&["fetch", "--prune"]),
        GitOp::Pull => network(&["pull"]),
        GitOp::Push => push(network),
        GitOp::Sync => sync(network),
        GitOp::Init => output(dir, &["init"]).map(drop),
        GitOp::Clone { url } => network(&["clone", "--", url]),
        GitOp::CreateBranch { name, base, switch } => {
            output(dir, &["check-ref-format", "--branch", name])?;
            let base = if base == "HEAD" { base.clone() } else { format!("refs/heads/{base}") };
            if *switch { output(dir, &["switch", "-c", name, &base]).map(drop) } else { output(dir, &["branch", "--", name, &base]).map(drop) }
        }
        GitOp::DeleteBranch { name, force } => output(dir, &["branch", if *force { "-D" } else { "-d" }, "--", name]).map(drop),
        GitOp::Abort(kind) => {
            if discover(dir).and_then(|(_, g)| sequence(&g)) != Some(*kind) {
                return Err(S::GitSequenceChanged.t().into());
            }
            output(dir, &[kind.command(), "--abort"]).map(drop)
        }
        GitOp::Switch(b) => switch_branch(dir, b).map_err(|(_, e)| e.message),
        GitOp::RetrySwitch { from, branch, choice } => retry_switch(dir, from, branch, *choice).map(drop),
        GitOp::PopStash(oid) => output(dir, &["stash", "pop", &stash_selector(dir, oid)?]).map(drop),
        GitOp::ApplyStash(oid) => {
            stash_selector(dir, oid)?;
            output(dir, &["stash", "apply", oid]).map(drop)
        }
        GitOp::DropStash(oid) => output(dir, &["stash", "drop", &stash_selector(dir, oid)?]).map(drop),
        GitOp::OpenRemote => return remote_url(dir).map(Some),
    }
    .map(|()| None)
}

fn execute(tab: flit_core::provider::TabId, dir: FsPath, mut op: GitOp) -> GitEvent {
    let path = std_path(&dir);
    let mut auth = None;
    let result = match &op {
        GitOp::Switch(branch) => {
            let _guard = BRANCH_OP.lock().unwrap_or_else(|e| e.into_inner());
            let from = current_branch(&path);
            match (switch_branch(&path, branch), from) {
                (Ok(()), _) => Ok(None),
                (Err((branch, e)), Some(from)) if e.blocked => return GitEvent::SwitchBlocked { tab, dir, from, branch: branch.to_owned() },
                (Err((_, e)), _) => Err(e.message),
            }
        }
        GitOp::RetrySwitch { from, branch, choice } => {
            let _guard = BRANCH_OP.lock().unwrap_or_else(|e| e.into_inner());
            retry_switch(&path, from, branch, *choice).map(|saved| {
                (!saved && *choice == SwitchChoice::Stash).then(|| GitOp::Switch(branch.clone()))
            }).map(|replacement| {
                if let Some(replacement) = replacement { op = replacement; }
                None
            })
        }
        GitOp::PopStash(_) | GitOp::ApplyStash(_) | GitOp::DropStash(_) => {
            let _guard = BRANCH_OP.lock().unwrap_or_else(|e| e.into_inner());
            run_op(&path, &op)
        }
        _ => run_op_with(&path, &op, &mut |args| {
            let result = output(&path, args).map(drop);
            auth = result.as_ref().err().and_then(|e| auth_event(tab, &dir, &op, args, e));
            result
        }),
    };
    if let Some(auth) = auth { return auth; }
    let (error, url) = match result {
        Ok(url) => (None, url),
        Err(e) => (Some(e), None),
    };
    GitEvent::Done { tab, op, error, url }
}

#[cfg(unix)]
fn file_id(_path: &Path, meta: &fs::Metadata) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn file_id(path: &Path, _meta: &fs::Metadata) -> io::Result<(u64, u64)> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};
    let file = fs::OpenOptions::new().access_mode(0).custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0).open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }?;
    Ok((info.dwVolumeSerialNumber.into(), (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)))
}

struct CloneDestination {
    path: PathBuf,
    id: (u64, u64),
    owned: bool,
}

impl CloneDestination {
    fn prepare(dir: &Path, url: &str) -> Result<Self, String> {
        let io_error = |e: io::Error| e.to_string();
        let mut name = git::repo_name(url).ok_or_else(|| io_error(io::ErrorKind::InvalidInput.into()))?;
        if !url.contains("://") && name.ends_with(".bundle") && dir.join(url).is_file() {
            name.truncate(name.len() - ".bundle".len());
        }
        if Path::new(&name).file_name() != Some(OsStr::new(&name)) {
            return Err(io_error(io::ErrorKind::InvalidInput.into()));
        }
        let path = dir.join(name);
        let mut active = CLONES.lock().unwrap_or_else(|e| e.into_inner());
        let owned = match fs::create_dir(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(io_error(e)),
        };
        let id = match fs::canonicalize(&path).and_then(|resolved| fs::metadata(&resolved).and_then(|meta| file_id(&resolved, &meta))) {
            Ok(id) => id,
            Err(e) => {
                if owned { let _ = fs::remove_dir(&path); }
                return Err(io_error(e));
            }
        };
        if active.contains(&id) { return Err(flit_core::tr!(S::GitCloneInProgress, path = path.display())); }
        active.push(id);
        Ok(Self { path, id, owned })
    }

    fn clean(&self, cancelled: bool) -> io::Result<()> {
        if !self.owned { return Ok(()) }
        let result = (|| {
            let meta = fs::symlink_metadata(&self.path)?;
            if !meta.is_dir() || file_id(&self.path, &meta)? != self.id { return Ok(()) }
            if cancelled { fs::remove_dir_all(&self.path) } else { fs::remove_dir(&self.path) }
        })();
        result.or_else(|e| if e.kind() == io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
    }
}

impl Drop for CloneDestination {
    fn drop(&mut self) {
        CLONES.lock().unwrap_or_else(|e| e.into_inner()).retain(|id| *id != self.id);
    }
}

fn index_lock(path: &Path) -> Option<GitIndexLock> {
    let meta = fs::symlink_metadata(path).ok().filter(|meta| meta.is_file())?;
    Some(GitIndexLock { path: path.to_owned(), file_id: file_id(path, &meta).ok()?, modified: meta.modified().ok()?, len: meta.len() })
}

fn new_index_lock(path: &Path, started: SystemTime) -> Option<GitIndexLock> {
    index_lock(path).filter(|lock| lock.modified > started)
}

fn remove_index_lock(lock: &GitIndexLock) -> Result<(), String> {
    if index_lock(&lock.path).as_ref() != Some(lock) {
        return Err(S::GitLockChanged.t().to_owned());
    }
    fs::remove_file(&lock.path).map_err(|e| e.to_string())
}

fn execute_tracked(tab: TabId, dir: FsPath, op: GitOp, job: GitJob, tx: &Sender<Reply>, wake: &Wake) -> GitEvent {
    let path = std_path(&dir);
    let mut auth = None;
    let mut cancelled = false;
    let mut stale_lock = None;
    let lock_path = matches!(op, GitOp::Pull | GitOp::Sync).then(|| discover(&path)).flatten()
        .and_then(|(_, gitdir)| fs::canonicalize(gitdir).ok())
        .map(|gitdir| gitdir.join("index.lock"))
        .filter(|lock| matches!(fs::symlink_metadata(lock), Err(e) if e.kind() == io::ErrorKind::NotFound));
    let started = SystemTime::now();
    let result = run_op_with(&path, &op, &mut |args| {
        if job.ctl.is_cancelled() {
            cancelled = true;
            return Err(S::GitOpCancelled.t().to_owned());
        }
        let clone = if let GitOp::Clone { url } = &op {
            Some(CloneDestination::prepare(&path, url)?)
        } else { None };
        let mut cmd = command(&path);
        cmd.arg(args[0]).arg("--progress").args(&args[1..]);
        if let Some(clone) = &clone { cmd.arg(&clone.path); }
        let result = process::run(cmd, &job.ctl, |progress| {
            if tx.send(GitEvent::Progress { tab, id: job.id, progress }).is_ok() { wake(); }
        });
        let result = result.map_err(|error| match error {
            process::RunError::Cancelled { forced } => {
                cancelled = true;
                if forced { stale_lock = lock_path.as_deref().and_then(|path| new_index_lock(path, started)); }
                if let Some(clone) = &clone {
                    if let Err(error) = clone.clean(true) {
                        cancelled = false;
                        return flit_core::tr!(S::GitCloneCleanupFailed, path = clone.path.display(), error = error);
                    }
                }
                S::GitOpCancelled.t().to_owned()
            }
            process::RunError::Failed(error) => {
                if let Some(clone) = &clone { let _ = clone.clean(false); }
                error
            }
        });
        auth = result.as_ref().err().filter(|_| !cancelled && !job.ctl.is_cancelled()).and_then(|error| match auth_event(tab, &dir, &op, args, error) {
            Some(GitEvent::AuthFailed { dir, github, .. }) => Some((dir, github)),
            _ => None,
        });
        result
    });
    let result = match result {
        Ok(_) => GitResult::Success,
        Err(_) if cancelled => stale_lock.map_or(GitResult::Cancelled, GitResult::CancelledWithLock),
        Err(error) => GitResult::Failed { error, auth },
    };
    GitEvent::TrackedDone { tab, id: job.id, op, result }
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

fn run(rx: Receiver<Job>, tx: Sender<Reply>, wake: Wake, transfers: &Mutex<Transfers>) {
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
                GitRequest::Branches { tab, gen, dir, create } => GitEvent::Branches { tab, gen, branches: branches(&std_path(&dir)), dir, create },
                GitRequest::Run { tab, dir, op } => {
                    start_operation(tab, dir, op, None, &tx, &wake, transfers);
                    continue;
                }
                GitRequest::RunTracked { tab, dir, op, job } => {
                    start_operation(tab, dir, op, Some(job), &tx, &wake, transfers);
                    continue;
                }
                GitRequest::RemoveIndexLock { tab, id, lock } => GitEvent::IndexLockRemoved { tab, id, error: remove_index_lock(&lock).err() },
            };
            if tx.send(reply).is_err() {
                return;
            }
            wake();
        }
    }
}

fn start_operation(tab: TabId, dir: FsPath, op: GitOp, job: Option<GitJob>, tx: &Sender<Reply>, wake: &Wake, transfers: &Mutex<Transfers>) {
    let mut transfers = transfers.lock().unwrap_or_else(|e| e.into_inner());
    if transfers.closed { return; }
    let (tx, wake) = (tx.clone(), wake.clone());
    let (worker_tx, worker_wake) = (tx.clone(), wake.clone());
    let _ = tx.send(GitEvent::Started { tab, op: op.clone() });
    wake();
    let (failed_op, id) = (op.clone(), job.as_ref().map(|j| j.id));
    let ctl = job.as_ref().map(|j| j.ctl.clone());
    let spawned = std::thread::Builder::new().name("flit-git-op".into()).spawn(move || {
        let reply = match job {
            Some(job) => execute_tracked(tab, dir, op, job, &worker_tx, &worker_wake),
            None => execute(tab, dir, op),
        };
        if worker_tx.send(reply).is_ok() { worker_wake(); }
    });
    if let Err(e) = spawned {
        let error = e.to_string();
        let reply = match id {
            Some(id) => GitEvent::TrackedDone { tab, id, op: failed_op, result: GitResult::Failed { error, auth: None } },
            None => GitEvent::Done { tab, op: failed_op, error: Some(error), url: None },
        };
        if tx.send(reply).is_ok() { wake(); }
    } else if let (Ok(worker), Some(ctl)) = (spawned, ctl) {
        transfers.active.retain(|(_, task)| !task.is_finished());
        transfers.active.push((ctl, worker));
    }
}

#[cfg(test)]
#[path = "../test/git.rs"]
mod tests;
