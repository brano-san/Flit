use flit_core::listing::{attr, Meta};
use flit_core::{FsError, FsErrorKind, FsPath, ListingBuilder};
use std::collections::HashSet;
use std::fmt::Write;
use std::io;
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};

use flit_core::path::NETWORK_URI as NETWORK_ROOT;
pub const DISCOVER_FOR: Duration = Duration::from_secs(13);
pub const ROUND: Duration = Duration::from_millis(1500);
pub const PROBE_WAIT: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(250);
const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::target-uri";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub uri: String,
}

pub trait Probe {
    fn list(&self, uri: &str, wait: Duration) -> io::Result<Output>;
    fn mount(&self, uri: &str, wait: Duration) -> io::Result<Output>;
    fn browse(&self, wait: Duration, each: &mut dyn FnMut(Option<&str>) -> bool) -> io::Result<()>;
    fn pause(&self, d: Duration);
    fn now(&self) -> Instant;
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

fn entries(listed: &str) -> impl Iterator<Item = Found> + '_ {
    listed.lines().filter_map(|line| {
        let attr_start = line.find("\tstandard::")?;
        let (name, _) = line[..attr_start].rsplit_once('\t')?.0.rsplit_once('\t')?;
        let attrs = &line[attr_start + 1..];
        let uri = attribute(attrs, "target-uri")?.trim();
        let shown = attribute(attrs, "display-name").map(str::trim).filter(|s| !s.is_empty()).unwrap_or(name);
        has_host(uri).then(|| Found { name: shown.to_owned(), uri: uri.to_owned() })
    })
}

pub fn parse_network(listed: &str) -> Vec<Found> {
    entries(listed).collect()
}

pub fn parse_shares(listed: &str) -> Vec<Found> {
    entries(listed).filter(|f| !f.name.ends_with('$')).collect()
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
    let (name, kind, host, address, port, record) = (unescape(f[3]), f[4], f[6], f[7], f[8], f.get(9..).map(|r| r.join(";")).unwrap_or_default());
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

pub fn discover(p: &dyn Probe, out: &mut ListingBuilder<'_>, family: usize) -> Result<(), FsError> {
    let until = p.now() + DISCOVER_FOR;
    let mut seen = HashSet::new();
    let mut gvfs = false;
    let why = loop {
        let left = until.saturating_duration_since(p.now());
        if left.is_zero() {
            if gvfs {
                return Ok(());
            }
            break Listed::Failed(io::ErrorKind::TimedOut.into());
        }
        let pause = match classify(p.list(NETWORK_ROOT, left.min(PROBE_WAIT))) {
            Listed::Text(text) => {
                gvfs = true;
                for item in parse_network(&text) {
                    add_once(out, &mut seen, item);
                }
                ROUND
            }
            _ if gvfs => ROUND,
            Listed::Failed(e) if e.kind() == io::ErrorKind::TimedOut => TICK,
            other => break other,
        };
        if !out.batch_done() {
            return Ok(());
        }
        p.pause(until.saturating_duration_since(p.now()).min(pause));
    };
    let browsed = p.browse(PROBE_WAIT, &mut |line| {
        if let Some(found) = line.and_then(parse_avahi) {
            add_once(out, &mut seen, found);
        }
        out.batch_done()
    });
    match (browsed, why) {
        (Ok(()), _) => Ok(()),
        (Err(_), Listed::Missing | Listed::Unsupported) => Err(FsError::no_network_browser(family)),
        (Err(_), Listed::Failed(e)) => Err(failed(&e)),
        (Err(e), _) => Err(failed(&e)),
    }
}

fn add_once(out: &mut ListingBuilder<'_>, seen: &mut HashSet<String>, item: Found) {
    if !seen.contains(&item.uri) {
        add(out, &item);
        seen.insert(item.uri);
    }
}

pub fn shares(p: &dyn Probe, uri: &str, out: &mut ListingBuilder<'_>, family: usize) -> Result<(), FsError> {
    let mut listed = classify(p.list(uri, PROBE_WAIT));
    if matches!(listed, Listed::NotMounted) {
        if !out.batch_done() {
            return Ok(());
        }
        listed = match p.mount(uri, PROBE_WAIT) {
            Ok(o) if o.status.success() || String::from_utf8_lossy(&o.stderr).contains("already mounted") => classify(p.list(uri, PROBE_WAIT)),
            Ok(o) if crate::places::needs_login(&o) => Listed::Denied,
            mounted => classify(mounted),
        };
    }
    match listed {
        Listed::Text(text) => {
            for share in parse_shares(&text) {
                add(out, &share);
            }
            out.batch_done();
            Ok(())
        }
        Listed::NotMounted | Listed::Denied => Err(FsError { kind: FsErrorKind::AccessDenied, code: 0 }),
        Listed::Missing | Listed::Unsupported => Err(FsError::no_network_browser(family)),
        Listed::Failed(e) => Err(failed(&e)),
    }
}

pub fn read_dir_with(p: &dyn Probe, dir: &FsPath, out: &mut ListingBuilder<'_>, family: usize) -> Result<(), FsError> {
    let parts: Vec<&[u8]> = dir.components().collect();
    match parts[..] {
        [] => discover(p, out, family),
        [server] => match std::str::from_utf8(server).ok().filter(|s| smb_server(s)) {
            Some(uri) => shares(p, uri, out, family),
            None => Err(FsError { kind: FsErrorKind::NotADirectory, code: 0 }),
        },
        _ => Err(FsError { kind: FsErrorKind::NotFound, code: 0 }),
    }
}

pub struct System;

fn quiet(mut cmd: std::process::Command) -> std::process::Command {
    cmd.env("LC_ALL", "C.UTF-8").env_remove("LANGUAGE");
    cmd
}

impl Probe for System {
    fn list(&self, uri: &str, wait: Duration) -> io::Result<Output> {
        let mut cmd = quiet(crate::tools::command("gio")?);
        cmd.args(["list", "-a", ATTRIBUTES, uri]);
        crate::places::run_within(cmd, wait)
    }

    fn mount(&self, uri: &str, wait: Duration) -> io::Result<Output> {
        let mut cmd = quiet(crate::tools::command("gio")?);
        cmd.args(["mount", "-a", uri]);
        crate::places::run_within(cmd, wait)
    }

    fn browse(&self, wait: Duration, each: &mut dyn FnMut(Option<&str>) -> bool) -> io::Result<()> {
        use std::io::BufRead;
        let mut cmd = quiet(crate::tools::command("avahi-browse")?);
        cmd.args(["-a", "-k", "-p", "-r", "-t"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| io::Error::other("avahi-browse"))?;
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::Builder::new().name("flit-avahi".into()).spawn(move || {
            let mut lines = std::io::BufReader::new(stdout);
            let mut line = Vec::new();
            while lines.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
                if tx.send(String::from_utf8_lossy(&line).trim_end().to_owned()).is_err() {
                    return;
                }
                line.clear();
            }
        })?;
        let until = Instant::now() + wait;
        let mut heard = false;
        let ended = loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break false;
            }
            match rx.recv_timeout(left.min(TICK)) {
                Ok(line) => {
                    heard = true;
                    if !each(Some(&line)) {
                        break false;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if !each(None) {
                        break false;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break true,
            }
        };
        if !ended {
            let _ = child.kill();
        }
        let out = child.wait_with_output()?;
        if ended && !out.status.success() && !heard {
            return Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
        }
        Ok(())
    }

    fn pause(&self, d: Duration) {
        std::thread::sleep(d);
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub fn read_dir(dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    read_dir_with(&System, dir, out, crate::tools::family(&crate::tools::os_release()))
}

#[cfg(test)]
#[path = "../test/network.rs"]
mod tests;
