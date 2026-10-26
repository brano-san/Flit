use crate::{fs, places};
use flit_core::provider::{FileOp, Launch};
use flit_core::ui::{Bitmap, IconRequest};
use flit_core::FsPath;
use flit_platform::{FileOps, Job, OpOutcome, PlatformError, Shell};
use flit_platform_linux::{host, run_op};
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TERMINALS: [&str; 16] = [
    "xdg-terminal-exec", "x-terminal-emulator", "gnome-terminal", "kgx", "ptyxis", "konsole", "xfce4-terminal", "mate-terminal", "tilix", "terminator", "lxterminal", "kitty", "alacritty", "foot", "wezterm", "xterm",
];
const HELPER_TIMEOUT: Duration = Duration::from_millis(500);

pub struct LinuxShell;

fn os(p: &FsPath) -> &OsStr {
    OsStr::from_bytes(p.as_bytes())
}

fn spawn_error(what: &str, e: &io::Error) -> PlatformError {
    PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("{what}: {e}"))
}

pub fn detach(mut cmd: Command) -> io::Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
    let mut child = cmd.spawn()?;
    std::thread::Builder::new().name("flit-reap".into()).spawn(move || {
        let _ = child.wait();
    })?;
    Ok(())
}

fn executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn on_path(program: &OsStr) -> Option<PathBuf> {
    if program.as_bytes().contains(&b'/') {
        return Some(PathBuf::from(program)).filter(|p| executable(p));
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(program)).find(|p| executable(p))
}

pub fn xdg_open(target: &OsStr) -> Result<(), PlatformError> {
    let program = on_path(OsStr::new("xdg-open")).ok_or_else(|| PlatformError::new(libc::ENOENT as i64, "xdg-open is not installed, so Flit cannot open files with other programs"))?;
    let mut cmd = Command::new(program);
    cmd.arg(target);
    detach(cmd).map_err(|e| spawn_error("xdg-open", &e))
}

pub fn terminal_program() -> Option<PathBuf> {
    let chosen = std::env::var_os("TERMINAL").filter(|t| !t.is_empty());
    chosen.iter().map(OsString::as_os_str).chain(TERMINALS.iter().map(OsStr::new)).find_map(on_path)
}

pub fn open_terminal(dir: &Path) -> Result<(), PlatformError> {
    let program = terminal_program().ok_or_else(|| PlatformError::new(libc::ENOENT as i64, "No terminal was found: set TERMINAL or install xdg-terminal-exec"))?;
    let mut cmd = Command::new(&program);
    cmd.current_dir(dir);
    detach(cmd).map_err(|e| spawn_error(&program.display().to_string(), &e))
}

pub fn split_args(text: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote, mut any) = (Vec::new(), String::new(), None, false);
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn launch(l: &Launch) -> Result<(), PlatformError> {
    match l {
        Launch::Terminal { admin: true, .. } => Err(PlatformError::new(-1, "Flit does not open terminals with elevated rights on Linux")),
        Launch::Terminal { dirs, .. } => dirs.iter().try_for_each(|d| open_terminal(Path::new(os(d)))),
        Launch::With { program, args } => {
            let mut cmd = Command::new(os(program));
            cmd.args(args.iter().map(os));
            detach(cmd).map_err(|e| spawn_error(&program.display(), &e))
        }
        Launch::Run { program, args, .. } if args.is_empty() && program.contains("://") => xdg_open(OsStr::new(program)),
        Launch::Run { program, args, dir } => {
            let mut cmd = Command::new(program);
            cmd.args(split_args(args));
            if let Some(d) = dir.as_ref().and_then(host) {
                cmd.current_dir(d);
            }
            detach(cmd).map_err(|e| spawn_error(program, &e))
        }
        Launch::Eject(_) => Err(PlatformError::unsupported("ejecting drives")),
    }
}

impl Shell for LinuxShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(LinuxShell)
    }

    fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
        None
    }

    fn thumbnail(&mut self, req: &IconRequest) -> Option<Bitmap> {
        let cache = crate::xdg::cache_dir().and_then(|d| d.parent().map(|p| p.join("thumbnails")));
        crate::thumb::thumbnail(host(&req.path)?, req.size, cache)
    }

    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }

    fn open(&mut self, path: &FsPath) -> Result<(), PlatformError> {
        xdg_open(os(path))
    }

    fn launch(&mut self, l: &Launch) -> Result<(), PlatformError> {
        launch(l)
    }
}

impl FileOps for LinuxShell {
    fn file_op(&mut self, op: &FileOp, job: &Job) -> OpOutcome {
        let trash = places::trash();
        let out = match (op, &trash) {
            (FileOp::Purge(pairs), Ok(t)) => places::purge(t, pairs),
            (FileOp::EmptyBin, Ok(t)) => places::empty(t, &places::storage_tops()),
            (FileOp::Purge(_) | FileOp::EmptyBin, Err(e)) => OpOutcome { items: Vec::new(), error: Some(e.clone()) },
            _ => run_op(op, job, trash.as_ref().ok()),
        };
        fs::rescan_blind();
        out
    }

    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        let own = OWN_FILES.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let read = |mime: &str| clipboard_bytes(mime).filter(|b| !b.is_empty());
        let (paths, cut) = match read("x-special/gnome-copied-files").or_else(|| read("text/uri-list")) {
            Some(bytes) => {
                let (paths, cut) = parse_file_list(&bytes);
                let ours = own.as_ref().filter(|(list, _)| parse_file_list(list.as_bytes()).0 == paths);
                (paths, cut || ours.is_some_and(|o| o.1))
            }
            None => own.map(|(list, cut)| (parse_file_list(list.as_bytes()).0, cut)).unwrap_or_default(),
        };
        if paths.is_empty() {
            return Err(PlatformError::new(-1, "the clipboard holds no files"));
        }
        Ok((paths, cut))
    }

    fn clear_clipboard(&mut self) {
        OWN_FILES.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

static OWN_FILES: std::sync::Mutex<Option<(String, bool)>> = std::sync::Mutex::new(None);

pub fn uri_list(paths: &[FsPath]) -> String {
    paths.iter().map(|p| format!("file://{}\r\n", flit_platform_linux::percent_encode(p.as_bytes()))).collect()
}

pub fn parse_file_list(bytes: &[u8]) -> (Vec<FsPath>, bool) {
    let mut lines = bytes.split(|&b| b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l)).filter(|l| !l.is_empty()).peekable();
    let cut = match lines.peek() {
        Some(&b"cut") => true,
        Some(&b"copy") => false,
        _ => return (lines.filter(|l| !l.starts_with(b"#")).filter_map(crate::instance::decode_uri).filter_map(|p| FsPath::parse_exact(&p)).collect(), false),
    };
    lines.next();
    (lines.filter_map(crate::instance::decode_uri).filter_map(|p| FsPath::parse_exact(&p)).collect(), cut)
}

pub fn remember_files(paths: &[FsPath], cut: bool) -> String {
    let list = uri_list(paths);
    *OWN_FILES.lock().unwrap_or_else(|e| e.into_inner()) = Some((list.clone(), cut));
    list
}

fn clipboard_bytes(mime: &str) -> Option<Vec<u8>> {
    let args: [&str; 7] = if wayland() { ["wl-paste", "--no-newline", "--type", mime, "", "", ""] } else { ["xclip", "-selection", "clipboard", "-target", mime, "-out", ""] };
    let args: Vec<&str> = args.into_iter().filter(|a| !a.is_empty()).collect();
    let child = helper(&args)?.stdout(Stdio::piped()).spawn().ok()?;
    wait_output(child, HELPER_TIMEOUT)
}

pub fn set_clipboard_files(list: &str) -> bool {
    let args: &[&str] = if wayland() { &["wl-copy", "--type", "text/uri-list"] } else { &["xclip", "-selection", "clipboard", "-target", "text/uri-list", "-in"] };
    let Some(mut cmd) = helper(args) else { return false };
    let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).process_group(0).spawn() else { return false };
    let written = child.stdin.take().is_some_and(|mut input| input.write_all(list.as_bytes()).is_ok());
    let _ = std::thread::Builder::new().name("flit-reap".into()).spawn(move || {
        let _ = child.wait();
    });
    written
}

fn wait_output(mut child: Child, limit: Duration) -> Option<Vec<u8>> {
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        out.read_to_end(&mut buf).map(|_| buf)
    });
    let end = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return reader.join().ok()?.ok(),
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

fn helper(args: &[&str]) -> Option<Command> {
    let mut cmd = Command::new(on_path(OsStr::new(args[0]))?);
    cmd.args(&args[1..]).stdin(Stdio::null()).stderr(Stdio::null());
    Some(cmd)
}

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub fn clipboard_text() -> Option<String> {
    let candidates: [&[&str]; 3] = [&["wl-paste", "--no-newline", "--type", "text/plain;charset=utf-8"], &["xclip", "-selection", "clipboard", "-out"], &["xsel", "--clipboard", "--output"]];
    let skip = if wayland() { 0 } else { 1 };
    candidates.iter().skip(skip).find_map(|args| {
        let mut cmd = helper(args)?;
        let child = cmd.stdout(Stdio::piped()).spawn().ok()?;
        wait_output(child, HELPER_TIMEOUT).and_then(|b| String::from_utf8(b).ok())
    })
}

pub fn set_clipboard_text(text: &str) -> bool {
    let candidates: [&[&str]; 3] = [&["wl-copy", "--type", "text/plain;charset=utf-8"], &["xclip", "-selection", "clipboard", "-in"], &["xsel", "--clipboard", "--input"]];
    let skip = if wayland() { 0 } else { 1 };
    candidates.iter().skip(skip).any(|args| {
        let Some(mut cmd) = helper(args) else { return false };
        let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).process_group(0).spawn() else { return false };
        let written = child.stdin.take().is_some_and(|mut input| input.write_all(text.as_bytes()).is_ok());
        let _ = std::thread::Builder::new().name("flit-reap".into()).spawn(move || {
            let _ = child.wait();
        });
        written
    })
}

pub fn parse_color_scheme(reply: &[u8]) -> Option<bool> {
    let text = String::from_utf8_lossy(reply);
    if text.contains("prefer-dark") {
        return Some(true);
    }
    if text.contains("prefer-light") || text.contains("'default'") {
        return Some(false);
    }
    let n = text.rsplit("uint32 ").next()?.trim_start().chars().next()?.to_digit(10)?;
    match n {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

fn gtk_theme_dark() -> Option<bool> {
    std::env::var("GTK_THEME").ok().map(|t| t.to_ascii_lowercase().ends_with(":dark") || t.to_ascii_lowercase().contains("-dark"))
}

fn session_bus() -> bool {
    std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() || crate::xdg::runtime_dir().is_some_and(|d| d.join("bus").exists())
}

fn probe_dark() -> Option<bool> {
    if let Some(dark) = gtk_theme_dark() {
        return Some(dark);
    }
    let portal = ["gdbus", "call", "--session", "--timeout", "1", "--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop", "--method", "org.freedesktop.portal.Settings.Read", "org.freedesktop.appearance", "color-scheme"];
    let gsettings = ["gsettings", "get", "org.gnome.desktop.interface", "color-scheme"];
    let ask = |args: &[&str]| helper(args).and_then(|mut c| c.stdout(Stdio::piped()).spawn().ok()).and_then(|child| wait_output(child, HELPER_TIMEOUT)).and_then(|r| parse_color_scheme(&r));
    session_bus().then(|| ask(&portal)).flatten().or_else(|| ask(&gsettings))
}

pub fn dark_preference(found: impl FnOnce(bool) + Send + 'static) {
    let _ = std::thread::Builder::new().name("flit-theme".into()).spawn(move || {
        if let Some(dark) = probe_dark() {
            found(dark);
        }
    });
}

#[cfg(test)]
#[path = "../test/shell.rs"]
mod tests;
