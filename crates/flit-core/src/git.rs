use crate::hash::FxHashMap;
use crate::path::FsPath;
use crate::provider::{OpCtl, OpId, TabId};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

const UNIX_TO_FILETIME_SECS: i64 = 11_644_473_600;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GitStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

impl GitStatus {
    pub fn letter(self) -> &'static str {
        match self {
            GitStatus::Modified => "M",
            GitStatus::Added => "A",
            GitStatus::Deleted => "D",
            GitStatus::Renamed => "R",
            GitStatus::Untracked => "U",
            GitStatus::Conflict => "!",
        }
    }

    fn rank(self) -> u8 {
        match self {
            GitStatus::Untracked => 0,
            GitStatus::Added => 1,
            GitStatus::Renamed => 2,
            GitStatus::Deleted => 3,
            GitStatus::Modified => 4,
            GitStatus::Conflict => 5,
        }
    }

    fn from_xy(x: u8, y: u8) -> Self {
        match (x, y) {
            (b'?', b'?') => GitStatus::Untracked,
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => GitStatus::Conflict,
            (b'A', _) => GitStatus::Added,
            (b'R' | b'C', _) => GitStatus::Renamed,
            (b'D', _) | (_, b'D') => GitStatus::Deleted,
            _ => GitStatus::Modified,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub time: i64,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitInfo {
    pub root: Option<FsPath>,
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub insertions: u64,
    pub deletions: u64,
    pub status: FxHashMap<Vec<u8>, GitStatus>,
    pub commits: FxHashMap<Vec<u8>, Arc<Commit>>,
    pub sequence: Option<Sequence>,
    pub conflicts: usize,
    pub stashes: Arc<[Stash]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stash {
    pub oid: String,
    pub message: String,
}

impl Stash {
    pub fn flit_branch(&self) -> Option<&str> {
        let (head, message) = self.message.strip_prefix("On ")?.split_once(": ")?;
        let branch = message.strip_prefix("Flit: ")?;
        (branch == head || (head == "(no branch)" && branch.len() == 7 && branch.bytes().all(|b| b.is_ascii_hexdigit()))).then_some(branch)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SwitchChoice {
    Merge,
    Stash,
    Discard,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sequence {
    Merge,
    Rebase,
    CherryPick,
}

impl Sequence {
    pub fn command(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    pub name: String,
    pub remote: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GitOp {
    Fetch,
    Pull,
    Push,
    Sync,
    Init,
    Clone { url: String },
    Switch(String),
    CreateBranch { name: String, base: String, switch: bool },
    DeleteBranch { name: String, force: bool },
    Abort(Sequence),
    OpenRemote,
    RetrySwitch { from: String, branch: String, choice: SwitchChoice },
    PopStash(String),
    ApplyStash(String),
    DropStash(String),
}

impl GitOp {
    pub fn is_network(&self) -> bool {
        matches!(self, Self::Fetch | Self::Pull | Self::Push | Self::Clone { .. } | Self::Sync)
    }
}

#[derive(Clone, Debug)]
pub struct GitJob {
    pub id: OpId,
    pub ctl: OpCtl,
}

impl PartialEq for GitJob {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GitRequest {
    Status { tab: TabId, gen: u64, dir: FsPath, commits: bool, changed: bool },
    Branches { tab: TabId, gen: u64, dir: FsPath, create: bool },
    Run { tab: TabId, dir: FsPath, op: GitOp },
    RunTracked { tab: TabId, dir: FsPath, op: GitOp, job: GitJob },
    RemoveIndexLock { tab: TabId, id: OpId, lock: GitIndexLock },
}

#[derive(Clone, Debug, PartialEq)]
pub enum GitEvent {
    Status { tab: TabId, gen: u64, info: Option<Arc<GitInfo>> },
    Branches { tab: TabId, gen: u64, dir: FsPath, create: bool, branches: Vec<Branch> },
    Started { tab: TabId, op: GitOp },
    Done { tab: TabId, op: GitOp, error: Option<String>, url: Option<String> },
    SwitchBlocked { tab: TabId, dir: FsPath, from: String, branch: String },
    AuthFailed { tab: TabId, dir: FsPath, op: GitOp, error: String, github: Option<GithubLogin> },
    Progress { tab: TabId, id: OpId, progress: GitProgress },
    TrackedDone { tab: TabId, id: OpId, op: GitOp, result: GitResult },
    IndexLockRemoved { tab: TabId, id: OpId, error: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum GitResult {
    Success,
    Cancelled,
    CancelledWithLock(GitIndexLock),
    Failed { error: String, auth: Option<(FsPath, Option<GithubLogin>)> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitIndexLock {
    pub path: PathBuf,
    pub file_id: (u64, u64),
    pub modified: SystemTime,
    pub len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitProgress {
    pub phase: GitPhase,
    pub percent: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitPhase {
    Counting,
    Compressing,
    Receiving,
    Resolving,
    Writing,
    Updating,
    Filtering,
    Unpacking,
}

impl GitPhase {
    pub fn label(self) -> &'static str {
        use crate::i18n::S;
        match self {
            Self::Counting => S::GitProgressCounting,
            Self::Compressing => S::GitProgressCompressing,
            Self::Receiving => S::GitProgressReceiving,
            Self::Resolving => S::GitProgressResolving,
            Self::Writing => S::GitProgressWriting,
            Self::Updating => S::GitProgressUpdating,
            Self::Filtering => S::GitProgressFiltering,
            Self::Unpacking => S::GitProgressUnpacking,
        }
        .t()
    }
}

pub fn progress_line(line: &str) -> Option<GitProgress> {
    let line = line.strip_prefix("remote: ").unwrap_or(line);
    let (phase, amount) = line.split_once(':')?;
    let phase = match phase {
        "Counting objects" | "Enumerating objects" => GitPhase::Counting,
        "Compressing objects" => GitPhase::Compressing,
        "Receiving objects" => GitPhase::Receiving,
        "Resolving deltas" => GitPhase::Resolving,
        "Writing objects" => GitPhase::Writing,
        "Updating files" | "Checking out files" => GitPhase::Updating,
        "Filtering content" => GitPhase::Filtering,
        "Unpacking objects" => GitPhase::Unpacking,
        _ => return None,
    };
    let percent = amount.trim_start().split_once('%')?.0.parse().ok()?;
    (percent <= 100).then_some(GitProgress { phase, percent })
}

#[derive(Clone, Debug, PartialEq)]
pub enum GithubLogin {
    Command(Vec<String>),
    Install(&'static str),
}

impl GitEvent {
    pub fn tab(&self) -> TabId {
        match self {
            GitEvent::Status { tab, .. }
            | GitEvent::Branches { tab, .. }
            | GitEvent::Started { tab, .. }
            | GitEvent::Done { tab, .. }
            | GitEvent::SwitchBlocked { tab, .. }
            | GitEvent::AuthFailed { tab, .. }
            | GitEvent::Progress { tab, .. }
            | GitEvent::TrackedDone { tab, .. }
            | GitEvent::IndexLockRemoved { tab, .. } => *tab,
        }
    }
}

pub fn github_remote(remote: &str) -> bool {
    remote_host(remote).is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
}

pub fn remote_host(remote: &str) -> Option<&str> {
    let remote = remote.trim();
    let authority = match remote.split_once("://") {
        Some((scheme, rest)) if ["https", "http", "ssh", "git"].iter().any(|s| scheme.eq_ignore_ascii_case(s)) => rest.split('/').next(),
        Some(_) => None,
        None => remote.split_once(':').map(|(host, _)| host),
    };
    let host = authority?.rsplit('@').next()?;
    let host = host.split_once(':').filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(host, _)| host);
    (!host.is_empty()).then_some(host)
}

pub fn filetime(unix_secs: i64) -> i64 {
    (unix_secs + UNIX_TO_FILETIME_SECS) * 10_000_000
}

pub fn head_branch(head: &[u8]) -> Option<String> {
    let head = head.trim_ascii();
    if let Some(r) = head.strip_prefix(b"ref:") {
        let r = r.trim_ascii();
        let name = r.strip_prefix(b"refs/heads/").unwrap_or(r);
        return Some(String::from_utf8_lossy(name).into_owned());
    }
    (head.len() >= 7 && head.iter().all(u8::is_ascii_hexdigit)).then(|| String::from_utf8_lossy(&head[..7]).into_owned())
}

pub fn gitdir_link(dot_git_file: &[u8]) -> Option<&[u8]> {
    dot_git_file.trim_ascii().strip_prefix(b"gitdir:").map(<[u8]>::trim_ascii)
}

fn child_of<'a>(path: &'a [u8], prefix: &[u8]) -> Option<(&'a [u8], bool)> {
    let rest = path.strip_prefix(prefix)?;
    let rest = rest.strip_suffix(b"/").unwrap_or(rest);
    if rest.is_empty() {
        return None;
    }
    match rest.iter().position(|&b| b == b'/') {
        Some(k) => Some((&rest[..k], true)),
        None => Some((rest, path.ends_with(b"/"))),
    }
}

fn merge(map: &mut FxHashMap<Vec<u8>, GitStatus>, name: &[u8], s: GitStatus, nested: bool) {
    let s = if nested && s != GitStatus::Untracked && s != GitStatus::Conflict { GitStatus::Modified } else { s };
    map.entry(name.to_vec())
        .and_modify(|cur| {
            if s.rank() > cur.rank() {
                *cur = s
            }
        })
        .or_insert(s);
}

pub fn status_entries(out: &[u8]) -> impl Iterator<Item = (u8, u8, &[u8])> {
    let mut parts = out.split(|&b| b == 0);
    std::iter::from_fn(move || {
        let rec = parts.find(|r| r.len() >= 4 && !r.starts_with(b"## "))?;
        let (x, y, path) = (rec[0], rec[1], &rec[3..]);
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            parts.next();
        }
        Some((x, y, path))
    })
}

pub fn parse_status(out: &[u8], prefix: &[u8]) -> FxHashMap<Vec<u8>, GitStatus> {
    let mut map = FxHashMap::default();
    for (x, y, path) in status_entries(out) {
        if let Some((name, nested)) = child_of(path, prefix) {
            merge(&mut map, name, GitStatus::from_xy(x, y), nested);
        }
    }
    map
}

pub fn parse_tracking(out: &[u8]) -> (Option<String>, u64, u64) {
    let header = out.split(|&b| b == 0).next().unwrap_or_default();
    let header = String::from_utf8_lossy(header);
    let Some((_, tracking)) = header.strip_prefix("## ").and_then(|h| h.split_once("...")) else { return (None, 0, 0) };
    let (upstream, counts) = tracking.strip_suffix(']').and_then(|s| s.rsplit_once(" [")).unwrap_or((tracking, ""));
    let (mut ahead, mut behind) = (0, 0);
    for field in counts.split(", ") {
        if let Some(n) = field.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or_default();
        } else if let Some(n) = field.strip_prefix("behind ") {
            behind = n.parse().unwrap_or_default();
        }
    }
    ((!upstream.is_empty() && counts != "gone").then(|| upstream.to_owned()), ahead, behind)
}

pub fn parse_shortstat(out: &[u8]) -> (u64, u64) {
    let (mut insertions, mut deletions) = (0, 0);
    let text = String::from_utf8_lossy(out);
    for field in text.split(',') {
        let mut words = field.split_whitespace();
        let count = words.next().and_then(|s| s.parse().ok()).unwrap_or_default();
        match words.next() {
            Some("insertion(+)" | "insertions(+)") => insertions = count,
            Some("deletion(-)" | "deletions(-)") => deletions = count,
            _ => {}
        }
    }
    (insertions, deletions)
}

pub struct LogParser {
    current: Option<Arc<Commit>>,
}

impl Default for LogParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LogParser {
    pub const FORMAT: &'static str = "--format=%x1e%H%x1f%h%x1f%an%x1f%ct%x1f%s";

    pub fn new() -> Self {
        Self { current: None }
    }

    pub fn feed(&mut self, field: &[u8], commits: &mut FxHashMap<Vec<u8>, Arc<Commit>>) {
        let field = field.strip_prefix(b"\n").unwrap_or(field);
        if let Some(header) = field.strip_prefix(b"\x1e") {
            let mut f = header.split(|&b| b == 0x1f).map(|s| String::from_utf8_lossy(s).into_owned());
            let (Some(hash), Some(short_hash), Some(author), Some(time), Some(message)) = (f.next(), f.next(), f.next(), f.next(), f.next()) else {
                self.current = None;
                return;
            };
            self.current = Some(Arc::new(Commit { hash, short_hash: short_hash.chars().take(7).collect(), author, time: filetime(time.trim().parse().unwrap_or(0)), message }));
            return;
        }
        let Some(c) = &self.current else { return };
        if let Some((name, _)) = child_of(field, b"") {
            commits.entry(name.to_vec()).or_insert_with(|| c.clone());
        }
    }
}

pub fn web_url(remote: &str) -> Option<String> {
    let r = remote.trim();
    let r = r.strip_suffix(".git").unwrap_or(r).trim_end_matches('/');
    let (host, path) = if let Some(rest) = r.strip_prefix("https://").or_else(|| r.strip_prefix("http://")) {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
        rest.split_once('/')?
    } else if let Some(rest) = r.strip_prefix("ssh://") {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?, path)
    } else {
        let rest = r.rsplit_once('@').map_or(r, |(_, h)| h);
        let (host, path) = rest.split_once(':')?;
        if host.contains('/') || host.len() < 2 {
            return None;
        }
        (host, path)
    };
    let path = path.trim_start_matches('/');
    (!host.is_empty() && !path.is_empty()).then(|| format!("https://{host}/{path}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Missing,
    Auth,
    Other,
}

const AUTH_SIGNS: [&str; 9] =
    ["could not read username", "could not read password", "terminal prompts disabled", "authentication failed", "permission denied (publickey", "host key verification failed", "invalid username or password", "invalid username or token", "returned error: 401"];

pub fn failure(error: &str) -> Failure {
    if error == crate::i18n::S::GitErrorMissing.en() {
        return Failure::Missing;
    }
    let lower = error.to_ascii_lowercase();
    if AUTH_SIGNS.iter().any(|s| lower.contains(s)) {
        Failure::Auth
    } else {
        Failure::Other
    }
}

pub fn op_command(op: &GitOp) -> Option<String> {
    Some(match op {
        GitOp::Fetch => "git fetch".to_owned(),
        GitOp::Pull => "git pull".to_owned(),
        GitOp::Push => "git push".to_owned(),
        GitOp::Sync => "git pull && git push".to_owned(),
        GitOp::Clone { url } => format!("git clone {url}"),
        _ => return None,
    })
}

pub fn repo_name(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches(['/', '\\']);
    let (scheme, address) = u.split_once("://").map_or((None, u), |(scheme, rest)| (Some(scheme), rest));
    let host_end = address.find(['/', '\\']).unwrap_or(address.len());
    let u = address[..host_end].rfind('@').map_or(address, |at| &address[at + 1..]);
    let u = if scheme.is_some() && !u.contains(['/', '\\']) { u.rsplit_once(':').filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).map_or(u, |(host, _)| host) } else { u };
    let u = u.strip_suffix("/.git").or_else(|| u.strip_suffix("\\.git")).unwrap_or(u).trim_end_matches(['/', '\\']);
    let name = u.rsplit(['/', ':', '\\']).next()?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let name = name.split(|c: char| c.is_ascii_whitespace() || (c as u32) < 32).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    (!name.is_empty() && name != "." && name != "..").then_some(name)
}

pub fn dropped_repo_url(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.contains(|c: char| c.is_whitespace() || c.is_control() || c == '\\') {
        return None;
    }
    let url = value.split(['?', '#']).next()?.trim_end_matches('/');
    let (authority, path, scp) = if let Some((scheme, rest)) = url.split_once("://") {
        if !["https", "http", "ssh", "git", "ftp"].iter().any(|s| scheme.eq_ignore_ascii_case(s)) {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        (authority, path, false)
    } else {
        let (host, path) = url.strip_prefix("git@")?.split_once(':')?;
        (host, path, true)
    };
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repo = segments.next();
    let host = authority.rsplit('@').next()?.split(':').next()?;
    if host.is_empty() || owner.is_empty() || path.split('/').any(|p| matches!(p, "" | "." | "..")) || repo_name(url).is_none() {
        return None;
    }
    let known = ["github.com", "gitlab.com", "codeberg.org", "bitbucket.org"].iter().any(|h| authority.eq_ignore_ascii_case(h));
    let https = url.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://"));
    (path.ends_with(".git") || (repo.is_some() && (scp || (https && known && segments.next().is_none())))).then_some(url)
}

#[cfg(test)]
#[path = "../test/git.rs"]
mod tests;
