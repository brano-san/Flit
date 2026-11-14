use crate::shell::on_path;
use crossbeam_channel::{unbounded, RecvTimeoutError};
use flit_core::i18n::S;
use flit_core::{tr_en, FsPath, Meta};
use flit_platform::PlatformError;
use flit_platform_linux::{percent_decode, stat_path};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const LIMIT: &str = "10000";
const WAIT: Duration = Duration::from_millis(100);
const TRACKER: [&str; 2] = ["localsearch", "tracker3"];
const BALOO: [&str; 2] = ["baloosearch6", "baloosearch"];
const RECOLL: [(&str, &[&str]); 2] = [("recollq", &[]), ("recoll", &["-t"])];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tracker,
    Baloo,
    Recoll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Engine {
    pub kind: Kind,
    pub program: PathBuf,
    pub lead: &'static [&'static str],
}

impl Engine {
    pub fn args(&self, folder: Option<&Path>, terms: &[String]) -> Vec<OsString> {
        let mut args: Vec<OsString> = self.lead.iter().map(OsString::from).collect();
        match self.kind {
            Kind::Tracker => {
                args.extend(["--disable-color", "--disable-snippets", "--limit", LIMIT, "--"].map(OsString::from));
                args.extend(terms.iter().map(|t| OsString::from(t.replace('"', ""))).filter(|t| !t.is_empty()));
            }
            Kind::Baloo => {
                args.extend(["--limit", LIMIT].map(OsString::from));
                if let Some(dir) = folder {
                    args.push("--directory".into());
                    args.push(dir.into());
                }
                args.push("--".into());
                args.extend(terms.iter().map(OsString::from));
            }
            Kind::Recoll => {
                args.extend(["-b", "-n", LIMIT].map(OsString::from));
                let joined = terms.join(" ");
                args.push(if joined.starts_with('-') { format!(" {joined}") } else { joined }.into());
            }
        }
        args
    }
}

fn kde(desktop: &str) -> bool {
    desktop.split(':').any(|d| d.eq_ignore_ascii_case("KDE"))
}

pub fn engines(desktop: &str, find: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<Engine> {
    let tracker = || TRACKER.iter().find_map(|p| Some(Engine { kind: Kind::Tracker, program: find(p)?, lead: &["search"] }));
    let baloo = || BALOO.iter().find_map(|p| Some(Engine { kind: Kind::Baloo, program: find(p)?, lead: &[] }));
    let recoll = || RECOLL.iter().find_map(|(p, lead)| Some(Engine { kind: Kind::Recoll, program: find(p)?, lead }));
    let order = if kde(desktop) { [baloo(), recoll(), tracker()] } else { [tracker(), recoll(), baloo()] };
    order.into_iter().flatten().collect()
}

pub fn terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut quoted) = (String::new(), false);
    for c in query.chars() {
        if c == '"' {
            quoted = !quoted;
        }
        if c.is_whitespace() && !quoted {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else if !c.is_control() {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn plain(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if line[i] == 0x1b && line.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < line.len() && !line[i].is_ascii_alphabetic() {
                i += 1;
            }
            i += 1;
            continue;
        }
        out.push(line[i]);
        i += 1;
    }
    out
}

pub fn parse_line(line: &[u8]) -> Option<PathBuf> {
    let line = plain(line);
    let line = line.trim_ascii();
    if let Some(uri) = line.strip_prefix(b"file://") {
        let uri = uri.split(u8::is_ascii_whitespace).next().unwrap_or_default();
        let path = percent_decode(uri.strip_prefix(b"localhost").unwrap_or(uri));
        return path.starts_with(b"/").then(|| PathBuf::from(OsStr::from_bytes(&path)));
    }
    line.starts_with(b"/").then(|| PathBuf::from(OsStr::from_bytes(line)))
}

fn run(engine: &Engine, args: &[OsString], keep_going: &dyn Fn() -> bool, found: &mut dyn FnMut(PathBuf)) -> Result<(), String> {
    let mut child = Command::new(&engine.program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("no output".into());
    };
    let (tx, rx) = unbounded::<Vec<u8>>();
    let reader = std::thread::Builder::new().name("flit-content-read".into()).spawn(move || {
        for line in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let errors = std::thread::Builder::new().name("flit-content-err".into()).spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        text
    });
    if reader.is_err() {
        let _ = child.kill();
    }
    loop {
        match rx.recv_timeout(WAIT) {
            Ok(line) => {
                if let Some(path) = parse_line(&line) {
                    found(path);
                }
            }
            Err(RecvTimeoutError::Timeout) if keep_going() => {}
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !keep_going() {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        return Ok(());
    }
    let text = errors.ok().and_then(|h| h.join().ok()).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    Err(text.lines().map(str::trim).rfind(|l| !l.is_empty()).map_or_else(|| status.to_string(), str::to_owned))
}

pub fn search_with(engines: &[Engine], folder: Option<&Path>, query: &str, keep_going: &dyn Fn() -> bool, found: &mut dyn FnMut(FsPath, Meta)) -> Result<(), PlatformError> {
    let Some(first) = engines.first() else { return Err(PlatformError::new(libc::ENOENT as i64, S::ErrorsEngineNoContentSearch.en())) };
    let terms = terms(query);
    if terms.is_empty() {
        return Ok(());
    }
    let mut failure = None;
    for engine in engines {
        let mut seen = HashSet::new();
        let mut any = false;
        let mut take = |path: PathBuf| {
            any = true;
            if folder.is_some_and(|f| !path.starts_with(f)) || !seen.insert(path.clone()) {
                return;
            }
            if let (Ok(meta), Some(at)) = (stat_path(&path), crate::xdg::fs_path(&path)) {
                found(at, meta);
            }
        };
        match run(engine, &engine.args(folder, &terms), keep_going, &mut take) {
            Ok(()) => return Ok(()),
            Err(_) if any => return Ok(()),
            Err(e) => {
                failure.get_or_insert((engine.program.file_name().unwrap_or_default().to_string_lossy().into_owned(), e));
            }
        }
        if !keep_going() {
            return Ok(());
        }
    }
    let (program, error) = failure.unwrap_or_else(|| (first.program.to_string_lossy().into_owned(), String::new()));
    Err(PlatformError::new(-1, tr_en!(S::ErrorsEngineContentSearchFailed, program = program, error = error)))
}

pub fn search(folder: Option<&FsPath>, query: &str, keep_going: &dyn Fn() -> bool, found: &mut dyn FnMut(FsPath, Meta)) -> Result<(), PlatformError> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let engines = engines(&desktop, &|p| on_path(OsStr::new(p)));
    search_with(&engines, folder.and_then(flit_platform_linux::host), query, keep_going, found)
}

#[cfg(test)]
#[path = "../test/content.rs"]
mod tests;
