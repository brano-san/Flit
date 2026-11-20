use flit_core::listing::{attr, Meta};
use flit_core::{FsError, FsErrorKind, FsPath, ListingBuilder};
use std::collections::{HashSet, VecDeque};
use std::fmt::Write;
use std::io;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use flit_core::path::NETWORK_URI as NETWORK_ROOT;
pub const DISCOVER_FOR: Duration = Duration::from_secs(6);
pub const ROUND: Duration = Duration::from_secs(1);
pub const PROBE_WAIT: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(250);
const SMB_ROOT: &str = "smb:///";
const SMB_GROUPS_IN_FLIGHT: usize = 4;
const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::target-uri";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub uri: String,
}

pub trait Probe {
    fn list(&self, uri: &str, wait: Duration) -> io::Result<Output>;
    fn mount(&self, uri: &str, wait: Duration) -> io::Result<Output>;
}

#[derive(Debug)]
pub enum Listed {
    Text(String),
    NotMounted,
    Denied,
    Unsupported,
    Missing,
    Failed(io::Error),
}

pub fn classify(out: io::Result<Output>) -> Listed {
    let out = match out {
        Ok(out) => out,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Listed::Missing,
        Err(e) => return Listed::Failed(e),
    };
    if out.status.success() {
        return Listed::Text(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    if said.contains("not mounted") {
        Listed::NotMounted
    } else if said.contains("not supported") {
        Listed::Unsupported
    } else if said.contains("Permission denied") || said.contains("Access denied") {
        Listed::Denied
    } else {
        Listed::Failed(io::Error::other(said.trim().to_owned()))
    }
}

fn attribute<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let tag = format!("standard::{key}=");
    let (at, _) = attrs.rmatch_indices(&tag).find(|(i, _)| *i == 0 || attrs[..*i].ends_with(' '))?;
    let rest = &attrs[at + tag.len()..];
    Some(&rest[..rest.find(" standard::").unwrap_or(rest.len())])
}

fn has_host(uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once("://") else { return false };
    let host = rest.split('/').next().unwrap_or_default().rsplit('@').next().unwrap_or_default();
    matches!(scheme, "smb" | "sftp" | "ssh" | "dav" | "davs" | "dav+sd" | "davs+sd" | "nfs" | "ftp" | "ftps" | "afp")
        && !host.is_empty() && !uri.contains('\\') && !uri.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn label(value: &str) -> String {
    value.chars().filter(|c| !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')).take(255).collect()
}

fn entries(listed: &str) -> impl Iterator<Item = (&str, &str, Option<&str>)> {
    listed.lines().filter_map(|line| {
        let attr_start = line.find("\tstandard::")?;
        let (name, _) = line[..attr_start].rsplit_once('\t')?.0.rsplit_once('\t')?;
        let attrs = &line[attr_start + 1..];
        let shown = attribute(attrs, "display-name").map(str::trim).filter(|s| !s.is_empty()).unwrap_or(name);
        Some((attribute(attrs, "name").unwrap_or(name), shown, attribute(attrs, "target-uri").map(str::trim)))
    })
}

pub fn parse_network(listed: &str) -> Vec<Found> {
    entries(listed).filter_map(|(name, shown, target)| {
        let uri = target.filter(|uri| *uri == SMB_ROOT || has_host(uri)).or_else(|| {
            (target.is_none_or(str::is_empty) && (name == "smb-root" || shown == "Windows Network")).then_some(SMB_ROOT)
        })?;
        Some(Found { name: label(shown), uri: uri.to_owned() })
    }).collect()
}

pub fn parse_shares(listed: &str) -> Vec<Found> {
    entries(listed).filter_map(|(name, shown, uri)| {
        let uri = uri.filter(|uri| has_host(uri))?;
        (!name.ends_with('$') && !shown.ends_with('$')).then(|| Found { name: shown.to_owned(), uri: uri.to_owned() })
    }).collect()
}

fn parse_smb_browse(listed: &str) -> impl Iterator<Item = Found> + '_ {
    entries(listed).filter_map(|(name, shown, target)| {
        let uri = match target {
            Some(uri) if has_host(uri) && smb_server(uri) => format!("{}/", uri.trim_end_matches('/')),
            None if !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '@']) && !name.chars().any(char::is_control) => format!("smb://{}/", encode_uri_part(name, false)),
            _ => return None,
        };
        Some(Found { name: label(shown), uri })
    })
}

fn unescape(label: &str) -> String {
    let b = label.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let digits = b.get(i + 1..i + 4).filter(|d| d.iter().all(u8::is_ascii_digit));
        match (b[i], digits) {
            (b'\\', Some(d)) => {
                let n = d.iter().fold(0u32, |n, c| n * 10 + u32::from(c - b'0'));
                out.push(u8::try_from(n).unwrap_or(b'?'));
                i += 4;
            }
            (b'\\', None) if i + 1 < b.len() => {
                out.push(b[i + 1]);
                i += 2;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn txt<'a>(record: &'a str, key: &str) -> Option<&'a str> {
    let tag = format!("\"{key}=");
    let at = record.find(&tag)? + tag.len();
    let rest = &record[at..];
    rest.find('"').map(|end| &rest[..end])
}

fn encode_uri_part(value: &str, keep_slash: bool) -> String {
    let mut encoded = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') || keep_slash && b == b'/' {
            encoded.push(char::from(b));
        } else {
            let _ = write!(encoded, "%{b:02X}");
        }
    }
    encoded
}

pub fn parse_avahi(line: &str) -> Option<Found> {
    let f: Vec<&str> = line.split(';').collect();
    if f.len() < 9 || f[0] != "=" {
        return None;
    }
    let (name, kind, host, address, port, record) = (label(&unescape(f[3])), f[4], f[6], f[7], f[8], f.get(9..).map(|r| r.join(";")).unwrap_or_default());
    let (scheme, default_port, with_path) = match kind {
        "_smb._tcp" => ("smb", 445, false),
        "_sftp-ssh._tcp" | "_ssh._tcp" => ("sftp", 22, false),
        "_webdav._tcp" => ("dav", 80, true),
        "_webdavs._tcp" => ("davs", 443, true),
        "_nfs._tcp" => ("nfs", 2049, true),
        "_ftp._tcp" => ("ftp", 21, true),
        _ => return None,
    };
    let host = match host.trim_end_matches('.') {
        "" => match address.parse::<std::net::IpAddr>().ok()? {
            std::net::IpAddr::V4(addr) => addr.to_string(),
            std::net::IpAddr::V6(addr) => format!("[{addr}]"),
        },
        h if h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')) => h.to_ascii_lowercase(),
        _ => return None,
    };
    let port = port.parse::<u16>().ok().filter(|&p| p != 0)?;
    let port = if port == default_port { String::new() } else { format!(":{port}") };
    let user = txt(&record, "u").filter(|u| with_path && !u.is_empty()).map_or_else(String::new, |u| format!("{}@", encode_uri_part(u, false)));
    let path = txt(&record, "path").filter(|p| with_path && p.starts_with('/')).unwrap_or("/");
    Some(Found { name, uri: format!("{scheme}://{user}{host}{port}{}", encode_uri_part(path, true)) })
}

pub fn smb_server(uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once("://") else { return false };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    scheme.eq_ignore_ascii_case("smb") && !host.is_empty() && path.trim_matches('/').is_empty()
}

pub fn server_place(uri: &str) -> Result<FsPath, flit_platform::PlatformError> {
    FsPath::network_server(uri.trim().as_bytes()).filter(|p| p.components().count() == 1).ok_or_else(|| flit_platform::PlatformError::new(-1, flit_core::i18n::S::SidebarDriveServerPrompt.en()))
}

fn add(out: &mut ListingBuilder<'_>, f: &Found) {
    out.push(f.uri.as_bytes(), Meta { attrs: attr::DIR, ..Default::default() });
    let i = out.listing.len() as u32 - 1;
    out.listing.set_label(i, &f.name);
    out.keys.relabel_last(out.listing.label(i));
}

fn failed(e: &io::Error) -> FsError {
    match e.kind() {
        io::ErrorKind::TimedOut => FsError::not_responding(),
        _ => FsError { kind: FsErrorKind::Unavailable, code: 0 },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Query {
    Network,
    Avahi,
    Smb(String),
}

enum Reply {
    Network(Listed),
    AvahiItem(Found),
    AvahiDone(io::Result<()>),
    Smb(Vec<Found>),
    Groups(Vec<Found>),
}

trait Discovery {
    fn now(&self) -> Instant;
    fn start(&mut self, query: Query, until: Instant);
    fn receive(&mut self, wait: Duration) -> Option<Reply>;
}

fn discover(p: &mut dyn Discovery, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let until = p.now() + DISCOVER_FOR;
    p.start(Query::Network, until);
    p.start(Query::Avahi, until);
    let mut seen = HashSet::new();
    let mut network = None;
    let mut avahi = None;
    let mut fallback = Vec::new();
    let mut next_round = None;
    let (mut answered, mut unchanged, mut last_count) = (false, 0, 0);
    let (mut smb_started, mut smb_pending) = (false, 0);
    let mut groups = VecDeque::new();
    loop {
        let now = p.now();
        if now >= until || unchanged == 2 || (network.is_some() && avahi.is_some()) {
            break;
        }
        if next_round.is_some_and(|next| now >= next) {
            next_round = None;
            p.start(Query::Network, until);
        }
        let wake = next_round.unwrap_or(until).min(until);
        let wait = wake.saturating_duration_since(p.now()).min(TICK);
        let reply = p.receive(wait);
        if p.now() >= until {
            break;
        }
        let items = match reply {
            Some(Reply::Network(Listed::Text(text))) => {
                let mut items = parse_network(&text);
                items.retain(|item| {
                    if item.uri != SMB_ROOT {
                        return true;
                    }
                    if !smb_started {
                        smb_started = true;
                        smb_pending += 1;
                        p.start(Query::Smb(item.uri.clone()), until);
                    }
                    false
                });
                for item in items {
                    add_once(out, &mut seen, item);
                }
                unchanged = if answered && seen.len() == last_count { unchanged + 1 } else { 0 };
                answered = true;
                last_count = seen.len();
                if unchanged < 2 {
                    next_round = Some(p.now() + ROUND);
                }
                Vec::new()
            }
            Some(Reply::Network(Listed::Failed(e))) if e.kind() == io::ErrorKind::TimedOut => {
                unchanged = 0;
                next_round = Some(p.now() + if answered { ROUND } else { TICK });
                Vec::new()
            }
            Some(Reply::Network(_)) if answered => {
                unchanged = 0;
                next_round = Some(p.now() + ROUND);
                Vec::new()
            }
            Some(Reply::Network(why)) => {
                network = Some(why);
                std::mem::take(&mut fallback)
            }
            Some(Reply::AvahiItem(item)) if network.is_some() => vec![item],
            Some(Reply::AvahiItem(item)) => {
                fallback.push(item);
                Vec::new()
            }
            Some(Reply::AvahiDone(result)) => {
                avahi = Some(result);
                Vec::new()
            }
            Some(Reply::Groups(items)) => {
                smb_pending -= 1;
                let mut queued = HashSet::new();
                groups.extend(items.into_iter().filter(|group| queued.insert(smb_host(&group.uri))));
                Vec::new()
            }
            Some(Reply::Smb(items)) => {
                smb_pending -= 1;
                items
            }
            None => Vec::new(),
        };
        for item in items {
            add_once(out, &mut seen, item);
        }
        while smb_pending < SMB_GROUPS_IN_FLIGHT {
            let Some(group) = groups.pop_front() else { break };
            p.start(Query::Smb(group.uri), until);
            smb_pending += 1;
        }
        if !out.batch_done() {
            return Ok(());
        }
    }
    if seen.is_empty() {
        for item in fallback {
            add_once(out, &mut seen, item);
        }
        if !seen.is_empty() && !out.batch_done() {
            return Ok(());
        }
    }
    if answered || !seen.is_empty() || avahi.as_ref().is_some_and(|v| !matches!(v, Err(e) if e.kind() == io::ErrorKind::NotFound)) {
        return Ok(());
    }
    match network {
        Some(Listed::Missing | Listed::Unsupported) if matches!(avahi.as_ref(), Some(Err(e)) if e.kind() == io::ErrorKind::NotFound) => Err(FsError::no_network_browser()),
        Some(Listed::Failed(e)) => Err(failed(&e)),
        Some(_) => Err(FsError { kind: FsErrorKind::Unavailable, code: 0 }),
        None => Ok(()),
    }
}

fn smb_host(uri: &str) -> String {
    let authority = uri.split_once("://").map_or("", |(_, rest)| rest).split('/').next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = match host.rsplit_once(':') {
        Some((name, port)) if port.parse::<u16>().is_ok() && (!name.contains(':') || name.ends_with(']')) => name,
        _ => host,
    };
    let host = flit_platform_linux::percent_decode(host.as_bytes());
    let lower = String::from_utf8_lossy(&host).trim_end_matches('.').to_lowercase();
    lower.strip_suffix(".local").unwrap_or(&lower).to_owned()
}

fn add_once(out: &mut ListingBuilder<'_>, seen: &mut HashSet<String>, item: Found) {
    let key = if smb_server(&item.uri) { format!("smb:{}", smb_host(&item.uri)) } else { item.uri.clone() };
    if seen.insert(key) {
        add(out, &item);
    }
}

fn mounted(o: &Output) -> bool {
    o.status.success() || String::from_utf8_lossy(&o.stderr).contains("already mounted")
}

pub fn mount_anonymous(p: &dyn Probe, uri: &str) -> bool {
    p.mount(uri, PROBE_WAIT).is_ok_and(|o| mounted(&o))
}

pub fn shares(p: &dyn Probe, uri: &str, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    match classify(p.list(uri, PROBE_WAIT)) {
        Listed::Text(text) => {
            for share in parse_shares(&text) {
                add(out, &share);
            }
            out.batch_done();
            Ok(())
        }
        Listed::NotMounted | Listed::Denied => Err(FsError { kind: FsErrorKind::AccessDenied, code: 0 }),
        Listed::Missing | Listed::Unsupported => Err(FsError { kind: FsErrorKind::Unavailable, code: 0 }),
        Listed::Failed(e) => Err(failed(&e)),
    }
}

fn read_server(p: &dyn Probe, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let mut parts = dir.components();
    let uri = parts.next().and_then(|server| std::str::from_utf8(server).ok()).filter(|s| smb_server(s));
    match (uri, parts.next()) {
        (Some(uri), None) => shares(p, uri, out),
        (_, None) => Err(FsError { kind: FsErrorKind::NotADirectory, code: 0 }),
        _ => Err(FsError { kind: FsErrorKind::NotFound, code: 0 }),
    }
}

pub struct System;

fn gio_list(mut cmd: Command, wait: Duration) -> io::Result<Output> {
    cmd.env("LC_ALL", "C.UTF-8");
    crate::places::run_within(cmd, wait)
}

fn list_args(uri: &str) -> [&str; 4] {
    ["list", "-a", ATTRIBUTES, uri]
}

fn mount_args(uri: &str) -> [&str; 4] {
    ["mount", "-a", "--", uri]
}

impl Probe for System {
    fn list(&self, uri: &str, wait: Duration) -> io::Result<Output> {
        gio_list(crate::places::gio_command(&list_args(uri))?, wait)
    }

    fn mount(&self, uri: &str, wait: Duration) -> io::Result<Output> {
        let cmd = crate::places::gio_command(&mount_args(uri))?;
        crate::places::run_within(cmd, wait)
    }
}

fn browse_smb(program: &Path, uri: &str, until: Instant) -> Vec<Found> {
    let budget = || PROBE_WAIT.min(until.saturating_duration_since(Instant::now()));
    let list = || classify(gio_list(crate::places::gio_at(program, &list_args(uri)), budget()));
    let mut listed = list();
    if matches!(listed, Listed::NotMounted) && !budget().is_zero() {
        let cmd = crate::places::gio_at(program, &mount_args(uri));
        if crate::places::run_within(cmd, budget()).is_ok_and(|o| mounted(&o)) && !budget().is_zero() {
            listed = list();
        }
    }
    match listed {
        Listed::Text(text) => parse_smb_browse(&text).collect(),
        _ => Vec::new(),
    }
}

fn browse_avahi(program: &Path, wait: Duration, tx: Sender<Reply>) -> io::Result<()> {
    let mut cmd = Command::new(program);
    cmd.args(["-a", "-k", "-p", "-r", "-t"]).env("LC_ALL", "C").env("LANGUAGE", "");
    crate::places::run_streaming(cmd, wait, move |line| {
        if let Some(item) = parse_avahi(line) {
            let _ = tx.send(Reply::AvahiItem(item));
        }
    }).and_then(|out| {
        if out.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_owned()))
        }
    })
}

struct SystemDiscovery {
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
}

impl SystemDiscovery {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }
}

impl Discovery for SystemDiscovery {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn start(&mut self, query: Query, until: Instant) {
        let tool = if query == Query::Avahi { "avahi-browse" } else { "gio" };
        let tx = self.tx.clone();
        let send_error = |error| {
            let reply = match query {
                Query::Network => Reply::Network(classify(Err(error))),
                Query::Avahi => Reply::AvahiDone(Err(error)),
                Query::Smb(_) => Reply::Smb(Vec::new()),
            };
            let _ = self.tx.send(reply);
        };
        let Ok(program) = crate::tools::require(tool) else {
            return send_error(io::ErrorKind::NotFound.into());
        };
        let job = query.clone();
        let spawned = std::thread::Builder::new().name("flit-network".into()).spawn(move || {
            let left = until.saturating_duration_since(Instant::now()).min(PROBE_WAIT);
            if left.is_zero() {
                return;
            }
            let reply = match job {
                Query::Network => Reply::Network(classify(gio_list(crate::places::gio_at(&program, &list_args(NETWORK_ROOT)), left))),
                Query::Avahi => Reply::AvahiDone(browse_avahi(&program, left, tx.clone())),
                Query::Smb(uri) if uri == SMB_ROOT => Reply::Groups(browse_smb(&program, &uri, until)),
                Query::Smb(uri) => Reply::Smb(browse_smb(&program, &uri, until)),
            };
            let _ = tx.send(reply);
        });
        if let Err(e) = spawned {
            send_error(e);
        }
    }

    fn receive(&mut self, wait: Duration) -> Option<Reply> {
        self.rx.recv_timeout(wait).ok()
    }
}

pub fn read_dir(dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    if dir.is_root() {
        discover(&mut SystemDiscovery::new(), out)
    } else {
        read_server(&System, dir, out)
    }
}

#[cfg(test)]
#[path = "../test/network.rs"]
mod tests;
