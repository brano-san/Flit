use crate::hash::FxHashMap;
use crate::path::FsPath;
use crate::provider::TabId;
use std::sync::Arc;

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
    pub author: String,
    pub time: i64,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitInfo {
    pub root: Option<FsPath>,
    pub branch: String,
    pub status: FxHashMap<Vec<u8>, GitStatus>,
    pub commits: FxHashMap<Vec<u8>, Arc<Commit>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GitOp {
    Fetch,
    Pull,
    Push,
    Init,
    Clone { url: String },
    Switch(String),
    OpenRemote,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GitRequest {
    Status { tab: TabId, gen: u64, dir: FsPath, commits: bool, changed: bool },
    Branches { tab: TabId, dir: FsPath },
    Run { tab: TabId, dir: FsPath, op: GitOp },
}

#[derive(Clone, Debug, PartialEq)]
pub enum GitEvent {
    Status { tab: TabId, gen: u64, info: Option<Arc<GitInfo>> },
    Branches { tab: TabId, branches: Vec<String> },
    Started { tab: TabId, op: GitOp },
    Done { tab: TabId, op: GitOp, error: Option<String>, url: Option<String> },
}

impl GitEvent {
    pub fn tab(&self) -> TabId {
        match self {
            GitEvent::Status { tab, .. } | GitEvent::Branches { tab, .. } | GitEvent::Started { tab, .. } | GitEvent::Done { tab, .. } => *tab,
        }
    }
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
    map.entry(name.to_vec()).and_modify(|cur| if s.rank() > cur.rank() { *cur = s }).or_insert(s);
}

pub fn parse_status(out: &[u8], prefix: &[u8]) -> FxHashMap<Vec<u8>, GitStatus> {
    let mut map = FxHashMap::default();
    let mut parts = out.split(|&b| b == 0);
    while let Some(rec) = parts.next() {
        if rec.len() < 4 {
            continue;
        }
        let (x, y, path) = (rec[0], rec[1], &rec[3..]);
        if matches!(x, b'R' | b'C') {
            parts.next();
        }
        if let Some((name, nested)) = child_of(path, prefix) {
            merge(&mut map, name, GitStatus::from_xy(x, y), nested);
        }
    }
    map
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
    pub const FORMAT: &'static str = "--format=%x1e%H%x1f%an%x1f%ct%x1f%s";

    pub fn new() -> Self {
        Self { current: None }
    }

    pub fn feed(&mut self, field: &[u8], commits: &mut FxHashMap<Vec<u8>, Arc<Commit>>) {
        let field = field.strip_prefix(b"\n").unwrap_or(field);
        if let Some(header) = field.strip_prefix(b"\x1e") {
            let mut f = header.split(|&b| b == 0x1f).map(|s| String::from_utf8_lossy(s).into_owned());
            let (hash, author, time, message) = (f.next().unwrap_or_default(), f.next().unwrap_or_default(), f.next().unwrap_or_default(), f.next().unwrap_or_default());
            self.current = Some(Arc::new(Commit { hash, author, time: filetime(time.trim().parse().unwrap_or(0)), message }));
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

pub fn repo_name(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches('/');
    let u = u.strip_suffix(".git").unwrap_or(u);
    let name = u.rsplit(['/', ':', '\\']).next()?;
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_owned())
}

#[cfg(test)]
#[path = "../test/git.rs"]
mod tests;
