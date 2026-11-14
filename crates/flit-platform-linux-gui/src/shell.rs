use crate::{fs, places};
use flit_core::i18n::S;
use flit_core::provider::{FileOp, Launch, OpenAsk};
use flit_core::ui::{Bitmap, IconKind, IconRequest};
use flit_core::path::PathKind;
use flit_core::FsPath;
use flit_platform::{FileOps, Job, OpOutcome, PlatformError, Shell};
use flit_platform_linux::{host, run_op};
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TERMINALS: [&str; 18] = [
    "xdg-terminal-exec", "x-terminal-emulator", "gnome-terminal", "kgx", "ptyxis", "konsole", "xfce4-terminal", "mate-terminal", "tilix", "terminator", "lxterminal", "kitty", "alacritty", "foot", "wezterm", "cosmic-term", "qterminal", "xterm",
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

pub fn no_xdg_open() -> String {
    let package = crate::tools::package_for("xdg-open", &crate::tools::os_release()).unwrap_or("xdg-utils");
    flit_core::tr_en!(flit_core::i18n::S::ErrorsOpsExtraNoXdgOpen, package = package)
}

pub fn xdg_open(target: &OsStr) -> Result<(), PlatformError> {
    let program = on_path(OsStr::new("xdg-open")).ok_or_else(|| PlatformError::new(libc::ENOENT as i64, no_xdg_open()))?;
    let mut cmd = Command::new(program);
    cmd.arg(target);
    crate::activation::pass(&mut cmd, crate::window::activation_token().as_deref());
    detach(cmd).map_err(|e| spawn_error("xdg-open", &e))
}

pub fn terminal_program() -> Option<PathBuf> {
    let chosen = std::env::var_os("TERMINAL").filter(|t| !t.is_empty());
    chosen.iter().map(OsString::as_os_str).chain(TERMINALS.iter().map(OsStr::new)).find_map(on_path)
}

enum DirArg {
    Cwd,
    Joined(&'static str),
    Pair(&'static str),
}

enum ExecArg {
    Rest(&'static str),
    Positional,
    Quoted(&'static str),
}

struct Style {
    lead: &'static [&'static str],
    dir: DirArg,
    exec: ExecArg,
}

const OPEN_SCRIPT: &str = r#"cd -- "$1" && exec "${SHELL:-/bin/sh}""#;
const RUN_SCRIPT: &str = r#"cd -- "$1" || exit; "$2"; echo; exec "${SHELL:-/bin/sh}""#;
const XTERM_STYLE: Style = Style { lead: &[], dir: DirArg::Cwd, exec: ExecArg::Rest("-e") };

fn terminal_style(name: &str) -> Option<Style> {
    let style = |lead, dir, exec| Some(Style { lead, dir, exec });
    match name {
        "gnome-terminal" | "kgx" => style(&[], DirArg::Joined("--working-directory="), ExecArg::Rest("--")),
        "ptyxis" => style(&["--new-window"], DirArg::Joined("--working-directory="), ExecArg::Rest("--")),
        "xfce4-terminal" | "mate-terminal" | "terminator" => style(&[], DirArg::Joined("--working-directory="), ExecArg::Rest("-x")),
        "tilix" | "lxterminal" => style(&[], DirArg::Joined("--working-directory="), ExecArg::Quoted("-e")),
        "konsole" => style(&[], DirArg::Pair("--workdir"), ExecArg::Rest("-e")),
        "qterminal" => style(&[], DirArg::Pair("--workdir"), ExecArg::Rest("-e")),
        "alacritty" | "cosmic-term" => style(&[], DirArg::Pair("--working-directory"), ExecArg::Rest("-e")),
        "kitty" => style(&[], DirArg::Joined("--directory="), ExecArg::Positional),
        "foot" | "footclient" => style(&[], DirArg::Joined("--working-directory="), ExecArg::Positional),
        "wezterm" => style(&["start"], DirArg::Pair("--cwd"), ExecArg::Rest("--")),
        "xdg-terminal-exec" => style(&[], DirArg::Cwd, ExecArg::Positional),
        "xterm" | "uxterm" | "urxvt" | "rxvt-unicode" | "st" => Some(XTERM_STYLE),
        _ => None,
    }
}

fn dispatcher(name: &str) -> bool {
    name == "xdg-terminal-exec" || name == "x-terminal-emulator" || name.ends_with(".wrapper")
}

pub fn real_terminal(program: &Path, find: impl Fn(&OsStr) -> Option<PathBuf>) -> PathBuf {
    let known = |p: &Path| p.file_name().and_then(OsStr::to_str).is_some_and(|n| terminal_style(n).is_some());
    let Ok(target) = std::fs::canonicalize(program) else { return program.to_owned() };
    let name = target.file_name().and_then(OsStr::to_str).unwrap_or_default();
    let real = match name.strip_suffix(".wrapper") {
        Some(base) => Some(target.with_file_name(base)).filter(|p| executable(p)).or_else(|| find(OsStr::new(base))),
        None => Some(target.clone()),
    };
    real.filter(|r| known(r)).unwrap_or_else(|| program.to_owned())
}

fn shell_quote(arg: &OsStr) -> OsString {
    let mut out = b"'".to_vec();
    for &b in arg.as_bytes() {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    OsString::from_vec(out)
}

pub fn terminal_args(program: &Path, dir: Option<&Path>, command: Option<Vec<OsString>>) -> Vec<OsString> {
    let name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let style = terminal_style(&name).unwrap_or(XTERM_STYLE);
    let mut args: Vec<OsString> = style.lead.iter().map(OsString::from).collect();
    match (dir, &style.dir) {
        (Some(d), DirArg::Joined(flag)) => {
            let mut a = OsString::from(flag);
            a.push(d);
            args.push(a);
        }
        (Some(d), DirArg::Pair(flag)) => args.extend([OsString::from(flag), d.into()]),
        _ => {}
    }
    let command = match (command, dir) {
        (Some(c), _) => c,
        (None, Some(d)) if dispatcher(&name) => ["sh", "-c", OPEN_SCRIPT, "flit"].into_iter().map(OsString::from).chain([d.into()]).collect(),
        (None, _) => return args,
    };
    match style.exec {
        ExecArg::Rest(flag) => {
            args.push(flag.into());
            args.extend(command);
        }
        ExecArg::Positional => args.extend(command),
        ExecArg::Quoted(flag) => {
            args.push(flag.into());
            args.push(command.iter().map(|c| shell_quote(c)).collect::<Vec<_>>().join(OsStr::new(" ")));
        }
    }
    args
}

fn spawn_terminal(dir: Option<&Path>, command: Option<Vec<OsString>>) -> Result<(), PlatformError> {
    let program = terminal_program().ok_or_else(|| PlatformError::new(libc::ENOENT as i64, "No terminal was found: set TERMINAL or install xdg-terminal-exec"))?;
    let program = real_terminal(&program, on_path);
    let mut cmd = Command::new(&program);
    cmd.args(terminal_args(&program, dir, command));
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    if let Some(t) = crate::window::activation_token() {
        cmd.env("XDG_ACTIVATION_TOKEN", &t).env("DESKTOP_STARTUP_ID", &t);
    }
    detach(cmd).map_err(|e| spawn_error(&program.display().to_string(), &e))
}

pub fn open_terminal(dir: &Path) -> Result<(), PlatformError> {
    spawn_terminal(Some(dir), None)
}

pub fn run_in_terminal(program: &Path) -> Result<(), PlatformError> {
    let dir = program.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("/"));
    if !std::fs::metadata(program).is_ok_and(|m| m.is_file()) {
        return Err(PlatformError::new(libc::ENOENT as i64, format!("{} is not a file", program.display())));
    }
    let command = ["sh", "-c", RUN_SCRIPT, "flit"].into_iter().map(OsString::from).chain([dir.into(), program.into()]).collect();
    spawn_terminal(Some(dir), Some(command))
}

const ROOT_SHELL: &str = "cd \"$1\" && exec \"${SHELL:-/bin/sh}\"";

pub fn root_shell(dir: &Path, sudo: bool) -> Vec<OsString> {
    if sudo {
        return ["sudo", "-s"].map(OsString::from).to_vec();
    }
    ["pkexec", "/bin/sh", "-c", ROOT_SHELL, "flit"].into_iter().map(OsString::from).chain([dir.into()]).collect()
}

pub fn open_root_terminal(dir: &Path) -> Result<(), PlatformError> {
    spawn_terminal(Some(dir), Some(root_shell(dir, on_path(OsStr::new("sudo")).is_some())))
}

pub fn run_in_terminal_argv(dir: Option<&Path>, argv: Vec<OsString>) -> Result<(), PlatformError> {
    spawn_terminal(dir, Some(argv))
}

pub fn elf_program(head: &[u8]) -> bool {
    if head.len() < 52 || &head[..4] != b"\x7fELF" {
        return false;
    }
    let (wide, le) = (head[4] == 2, head[5] == 1);
    let u16_at = |at: usize| head.get(at..at + 2).map(|b| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |at: usize| head.get(at..at + 4).map(|b| if le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) });
    let u64_at = |at: usize| head.get(at..at + 8).map(|b| {
        let a: [u8; 8] = b.try_into().unwrap_or_default();
        if le { u64::from_le_bytes(a) } else { u64::from_be_bytes(a) }
    });
    match u16_at(16) {
        Some(2) => true,
        Some(3) => {
            let (phoff, phentsize, phnum) = if wide { (u64_at(32).map(|v| v as usize), u16_at(54), u16_at(56)) } else { (u32_at(28).map(|v| v as usize), u16_at(42), u16_at(44)) };
            let (Some(phoff), Some(size), Some(count)) = (phoff, phentsize, phnum) else { return false };
            (0..count as usize).any(|k| phoff.checked_add(k * size as usize).and_then(u32_at) == Some(3))
        }
        _ => false,
    }
}

pub fn runs_directly(path: &Path) -> bool {
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    if unsafe { libc::access(c.as_ptr(), libc::X_OK) } != 0 {
        return false;
    }
    let mut head = vec![0u8; 4096];
    let Ok(n) = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)) else { return false };
    head.truncate(n);
    let appimage = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("appimage"));
    elf_program(&head) || appimage && head.starts_with(b"\x7fELF")
}

fn program_file(path: &Path) -> bool {
    let mut head = vec![0u8; 4096];
    let Ok(n) = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)) else { return false };
    head.truncate(n);
    let appimage = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("appimage"));
    elf_program(&head) || appimage && head.starts_with(b"\x7fELF")
}

pub fn needs_exec_bit(path: &Path) -> bool {
    let program = std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 == 0) && program_file(path);
    program || launcher_app(path).is_some() && !crate::apps::trusted_launcher(path, &crate::apps::data_dirs())
}

const LAUNCHER_MAX: u64 = 1 << 20;
const SAID_MAX: usize = 4096;
const SAID_SHOWN: usize = 160;
const QUIET_START: Duration = Duration::from_millis(600);
const DRAIN_WAIT: Duration = Duration::from_millis(200);

fn is_launcher(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("desktop"))
}

fn launcher_text(path: &Path) -> Option<String> {
    let small = is_launcher(path) && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= LAUNCHER_MAX);
    small.then(|| std::fs::read_to_string(path).ok()).flatten()
}

fn launcher_app(path: &Path) -> Option<crate::apps::Entry> {
    crate::apps::parse_desktop("", path, &launcher_text(path)?, "")
}

pub fn launcher_folder(path: &Path) -> Option<PathBuf> {
    let url = crate::apps::link_url(&launcher_text(path)?)?;
    let local = if url.starts_with('/') { url.into_bytes() } else { crate::instance::decode_uri(url.as_bytes())? };
    Some(PathBuf::from(OsString::from_vec(local))).filter(|p| p.is_dir())
}

fn open_launcher(path: &Path) -> Option<Result<(), PlatformError>> {
    let text = launcher_text(path)?;
    if let Some(url) = crate::apps::link_url(&text) {
        return Some(xdg_open(OsStr::new(&url)));
    }
    crate::apps::parse_desktop("", path, &text, "")?;
    if !crate::apps::trusted_launcher(path, &crate::apps::data_dirs()) {
        return None;
    }
    let token = crate::window::activation_token();
    Some(crate::apps::launch(path, &[], token.as_deref()).map_err(|e| spawn_error(&path.display().to_string(), &e)))
}

pub fn executable_script(path: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    if unsafe { libc::access(c.as_ptr(), libc::X_OK) } != 0 {
        return false;
    }
    let mut head = [0u8; 2];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == *b"#!"
}

pub fn open_question(path: &Path, entries: &[crate::apps::Entry], assoc: &crate::apps::Assoc) -> Option<OpenAsk> {
    if is_launcher(path) || !std::fs::metadata(path).is_ok_and(|m| m.is_file()) || runs_directly(path) {
        return None;
    }
    if executable_script(path) {
        return Some(OpenAsk::Script);
    }
    if entries.is_empty() {
        return None;
    }
    let mime = crate::mime::mime_of_file(path);
    crate::apps::handlers_by_lineage(&mime, entries, assoc, crate::mime::parents).is_empty().then(|| OpenAsk::NoApp { mime, apps: crate::apps::choices(entries) })
}

fn open_without_xdg_open(path: &Path) -> Option<Result<(), PlatformError>> {
    let c = crate::apps::catalog();
    let mime = crate::mime::mime_of_file(path);
    let app = crate::apps::handlers_by_lineage(&mime, &c.entries, &c.assoc, crate::mime::parents).first().map(|e| e.path.clone())?;
    let token = crate::window::activation_token();
    Some(crate::apps::launch(&app, &[path.to_path_buf()], token.as_deref()).map_err(|e| spawn_error(&app.display().to_string(), &e)))
}

fn run_command(argv: &[String], env: &[(String, String)], dir: Option<&Path>) -> Result<(), PlatformError> {
    let Some((program, args)) = argv.split_first() else { return Ok(()) };
    let mut cmd = Command::new(program);
    cmd.args(args).envs(env.iter().map(|(k, v)| (k, v)));
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    crate::activation::pass(&mut cmd, crate::window::activation_token().as_deref());
    detach(cmd).map_err(|e| spawn_error(program, &e))
}

fn tool_missing(tool: &str) -> PlatformError {
    let package = crate::tools::package_for(tool, &crate::tools::os_release()).unwrap_or(tool);
    PlatformError::new(libc::ENOENT as i64, flit_core::tr_en!(flit_core::i18n::S::ErrorsOpsExtraNoTool, tool = tool, package = package))
}

fn print_files(files: &[PathBuf]) -> Result<(), PlatformError> {
    let program = on_path(OsStr::new("lp")).ok_or_else(|| tool_missing("lp"))?;
    let out = Command::new(program).args(files).stdin(Stdio::null()).output().map_err(|e| spawn_error("lp", &e))?;
    if out.status.success() {
        return Ok(());
    }
    Err(PlatformError::new(-1, format!("lp: {}", first_words(&out.stderr))))
}

pub fn other_file_manager<'a>(entries: &'a [crate::apps::Entry], assoc: &crate::apps::Assoc) -> Option<&'a crate::apps::Entry> {
    let flit = |e: &crate::apps::Entry| e.id == flit_core::mimeapps::FLIT_DESKTOP || split_args(&e.exec).first().is_some_and(|p| Path::new(p).file_name() == Some(OsStr::new("flit")));
    crate::apps::handlers(flit_core::mimeapps::FOLDER, entries, assoc).into_iter().find(|e| !flit(e) && e.categories.iter().any(|c| c == "FileManager"))
}

fn open_in_file_manager(dir: &Path) -> Result<(), PlatformError> {
    let c = crate::apps::catalog();
    let app = other_file_manager(&c.entries, &c.assoc).map(|e| e.path.clone()).ok_or_else(|| PlatformError::new(-1, flit_core::i18n::S::ErrorsOpsExtraNoFileManager.en()))?;
    let token = crate::window::activation_token();
    crate::apps::launch(&app, &[dir.to_path_buf()], token.as_deref()).map_err(|e| spawn_error(&app.display().to_string(), &e))
}

pub fn loop_device(said: &str) -> Option<PathBuf> {
    let at = said.find(" as /dev/")? + 4;
    Some(PathBuf::from(said[at..].trim().trim_end_matches('.')))
}

fn udisksctl_message(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.contains(".Error.DeviceBusy") || text.ends_with("target is busy") {
        return S::ErrorsFsDriveBusy.en().to_owned();
    }
    match text.split_once("GDBus.Error:") {
        Some((_, rest)) => rest.split_once(": ").map_or(rest, |(_, why)| why).trim().to_owned(),
        None => text.to_owned(),
    }
}

fn mount_image(file: &Path) -> io::Result<PathBuf> {
    let udisks = on_path(OsStr::new("udisksctl")).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, tool_missing("udisksctl").message))?;
    let out = Command::new(udisks).args(["loop-setup", "-r", "-f"]).arg(file).stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(io::Error::other(udisksctl_message(&out.stderr)));
    }
    let device = loop_device(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| io::Error::other("udisksctl did not name the loop device"))?;
    crate::udisks::mount(&device).or_else(|e| {
        let table = flit_platform_linux::mounts::read()?;
        table.iter().rev().find(|m| m.source == device.as_os_str().as_bytes()).map(|m| m.path().to_path_buf()).ok_or(e)
    })
}

pub fn install_fonts(fonts: &[FsPath], dir: Option<PathBuf>) -> OpOutcome {
    let Some(dir) = dir else { return OpOutcome { items: Vec::new(), error: Some(PlatformError::new(-1, "no HOME or XDG_DATA_HOME")) } };
    let (mut out, mut errors) = (OpOutcome::default(), Vec::new());
    for font in fonts {
        let shown = font.display().into_owned();
        let Some((src, name)) = host(font).and_then(|p| Some((p, p.file_name()?))) else { continue };
        if !flit_core::install::is_font(font.file_name()) {
            errors.push(flit_core::tr_en!(flit_core::i18n::S::ErrorsOpsExtraNotFont, path = shown));
            continue;
        }
        let to = dir.join(name);
        let same = std::fs::canonicalize(src).ok().is_some_and(|s| std::fs::canonicalize(&to).ok() == Some(s));
        let copied = if same { Ok(()) } else { std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(src, &to)).map(drop) };
        match (copied, crate::xdg::fs_path(&to)) {
            (Ok(()), Some(target)) => out.items.push((font.clone(), target)),
            (Ok(()), None) => {}
            (Err(e), _) => errors.push(flit_core::tr_en!(flit_core::i18n::S::ErrorsOpsExtraFont, path = shown, error = e)),
        }
    }
    if !out.items.is_empty() {
        if let Some(fc) = on_path(OsStr::new("fc-cache")) {
            let mut cmd = Command::new(fc);
            cmd.arg("-f").arg(&dir);
            let _ = detach(cmd);
        }
    }
    out.error = (!errors.is_empty()).then(|| PlatformError::new(-1, errors.join("\n")));
    out
}

#[derive(Default)]
struct Said {
    bytes: Vec<u8>,
    closed: usize,
}

fn drain(mut pipe: impl Read + Send + 'static, said: std::sync::Arc<(std::sync::Mutex<Said>, std::sync::Condvar)>) {
    let _ = std::thread::Builder::new().name("flit-drain".into()).spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            let mut s = said.0.lock().unwrap_or_else(|e| e.into_inner());
            let room = SAID_MAX.saturating_sub(s.bytes.len()).min(n);
            s.bytes.extend_from_slice(&buf[..room]);
        }
        said.0.lock().unwrap_or_else(|e| e.into_inner()).closed += 1;
        said.1.notify_all();
    });
}

pub fn first_words(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    match line.char_indices().nth(SAID_SHOWN) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

pub fn watch_start(mut child: Child, limit: Duration) -> Option<String> {
    let said = std::sync::Arc::new((std::sync::Mutex::new(Said::default()), std::sync::Condvar::new()));
    let mut opened = 0;
    if let Some(out) = child.stdout.take() {
        drain(out, said.clone());
        opened += 1;
    }
    if let Some(err) = child.stderr.take() {
        drain(err, said.clone());
        opened += 1;
    }
    let end = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let guard = said.0.lock().unwrap_or_else(|e| e.into_inner());
                let (guard, _) = said.1.wait_timeout_while(guard, DRAIN_WAIT, |s| s.closed < opened).unwrap_or_else(|e| e.into_inner());
                let words = first_words(&guard.bytes);
                return (!status.success() || !words.is_empty()).then_some(words);
            }
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = std::thread::Builder::new().name("flit-reap".into()).spawn(move || {
                    let _ = child.wait();
                });
                return None;
            }
            Err(_) => return None,
        }
    }
}

pub fn console_hint(name: &str, words: &str) -> String {
    if words.is_empty() {
        flit_core::tr_en!(flit_core::i18n::S::ErrorsOpsExtraEndedAtOnce, name = name)
    } else {
        flit_core::tr_en!(flit_core::i18n::S::ErrorsOpsExtraEndedSaying, name = name, words = words)
    }
}

fn run_directly(path: &Path) -> Result<(), PlatformError> {
    let mut cmd = Command::new(path);
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        cmd.current_dir(d);
    }
    if let Some(t) = crate::window::activation_token() {
        cmd.env("XDG_ACTIVATION_TOKEN", &t).env("DESKTOP_STARTUP_ID", &t);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let child = cmd.spawn().map_err(|e| spawn_error(&path.display().to_string(), &e))?;
    match watch_start(child, QUIET_START) {
        Some(words) => {
            let name = flit_core::tr!(flit_core::i18n::S::UiQuoted, name = path.file_name().unwrap_or_default().to_string_lossy());
            Err(PlatformError::new(-1, console_hint(&name, &words)))
        }
        None => Ok(()),
    }
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

pub fn opened_documents(l: &Launch) -> Vec<PathBuf> {
    let files = match l {
        Launch::With { args, .. } | Launch::Custom { files: args, .. } => args.as_slice(),
        Launch::Display(file) => std::slice::from_ref(file),
        _ => &[],
    };
    files.iter().filter_map(|f| host(f).map(Path::to_path_buf)).collect()
}

fn launch(l: &Launch) -> Result<(), PlatformError> {
    match l {
        Launch::Terminal { dirs, admin: true } => dirs.iter().try_for_each(|d| open_root_terminal(Path::new(os(d)))),
        Launch::Terminal { dirs, .. } => dirs.iter().try_for_each(|d| open_terminal(Path::new(os(d)))),
        Launch::RunInTerminal { program } => run_in_terminal(Path::new(os(program))),
        Launch::Execute(program) => run_directly(Path::new(os(program))),
        Launch::Display(file) => xdg_open(os(file)),
        Launch::Print(files) => print_files(&files.iter().filter_map(|f| host(f).map(Path::to_path_buf)).collect::<Vec<_>>()),
        Launch::FileManager(dir) => open_in_file_manager(Path::new(os(dir))),
        Launch::Command { runs, dir } => runs.iter().try_for_each(|(argv, env)| run_command(argv, env, dir.as_ref().and_then(host))),
        Launch::With { program, args } if program.as_bytes().ends_with(b".desktop") => {
            let files: Vec<PathBuf> = args.iter().filter_map(|a| host(a).map(Path::to_path_buf)).collect();
            let token = crate::window::activation_token();
            crate::apps::launch(Path::new(os(program)), &files, token.as_deref()).map_err(|e| spawn_error(&program.display(), &e))
        }
        Launch::With { program, args } => {
            let mut cmd = Command::new(os(program));
            cmd.args(args.iter().map(os));
            crate::activation::pass(&mut cmd, crate::window::activation_token().as_deref());
            detach(cmd).map_err(|e| spawn_error(&program.display(), &e))
        }
        Launch::Run { program, args, .. } if args.is_empty() && program.contains("://") => xdg_open(OsStr::new(program)),
        Launch::Run { program, args, dir } => {
            let mut cmd = Command::new(program);
            cmd.args(split_args(args));
            if let Some(d) = dir.as_ref().and_then(host) {
                cmd.current_dir(d);
            }
            crate::activation::pass(&mut cmd, crate::window::activation_token().as_deref());
            detach(cmd).map_err(|e| spawn_error(program, &e))
        }
        Launch::SetDefault { program, mime } => crate::apps::set_default(Path::new(os(program)), mime).map_err(|e| spawn_error("mimeapps.list", &e)),
        Launch::Custom { command, files, mime, remember } => {
            let files: Vec<PathBuf> = files.iter().filter_map(|a| host(a).map(Path::to_path_buf)).collect();
            crate::apps::run_custom(command, &files, mime, *remember, crate::window::activation_token().as_deref()).map_err(|e| spawn_error(command, &e))
        }
        Launch::ChooseApp { .. } => Err(PlatformError::unsupported("choosing an application outside a window")),
        Launch::Eject(drive) => crate::udisks::eject(Path::new(os(drive))).map_err(|e| crate::places::eject_error(Path::new(os(drive)), &e)),
        Launch::Unmount(drive) => crate::udisks::unmount(Path::new(os(drive))).map_err(|e| crate::places::drive_error(S::ErrorsFsUnmountFailed, Path::new(os(drive)), &e)),
        Launch::Mount(_) | Launch::Unlock { .. } => Err(PlatformError::unsupported("mounting outside the shell thread")),
        Launch::Disconnect(Some(drive)) => crate::places::disconnect(Path::new(os(drive))).map_err(|e| crate::places::drive_error(S::ErrorsFsDisconnectFailed, Path::new(os(drive)), &e)),
        Launch::MapDrive(_) | Launch::Disconnect(None) | Launch::ConnectServer(_) => Err(PlatformError::unsupported("mapping network drives")),
        Launch::Reveal { .. } => Err(PlatformError::unsupported("revealing outside the shell thread")),
        Launch::Wallpaper(image) => crate::wallpaper::set(Path::new(os(image))),
    }
}

impl Shell for LinuxShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(LinuxShell)
    }

    fn icon(&mut self, req: &IconRequest) -> Option<Bitmap> {
        if let Some(target) = (req.kind == IconKind::File).then(|| link_target_kind(&req.path)).flatten() {
            return link_icon(req, target);
        }
        let picture = (req.kind == IconKind::Thumbnail).then(|| self.thumbnail(req)).flatten();
        let own = || (req.kind == IconKind::File).then(|| host(&req.path).and_then(launcher_text)).flatten().and_then(|t| crate::apps::launcher_icon(&t));
        picture.or_else(|| own().and_then(|name| crate::apps::icon_image(&name, req.size))).or_else(|| crate::icons::icon(&icon_names(req), req.size))
    }

    fn thumbnail(&mut self, req: &IconRequest) -> Option<Bitmap> {
        let cache = crate::xdg::cache_dir().and_then(|d| d.parent().map(|p| p.join("thumbnails")));
        let path = host(&req.path)?;
        crate::thumb::thumbnail(path, req.size, cache).or_else(|| crate::thumbnailers::thumbnail(path, req.size)).or_else(|| poster(path, req.size))
    }

    fn preview_image(&mut self, path: &FsPath, max: u32) -> Option<Bitmap> {
        let path = host(path)?;
        crate::thumb::image(path, max).or_else(|| crate::thumbnailers::thumbnail(path, max))
    }

    fn poster(&mut self, path: &FsPath, size: u32) -> Option<Bitmap> {
        poster(host(path)?, size)
    }

    fn type_name(&mut self, ext: &[u8]) -> Option<String> {
        crate::mime::type_name(ext)
    }

    fn open(&mut self, path: &FsPath) -> Result<(), PlatformError> {
        let p = Path::new(os(path));
        if runs_directly(p) {
            return run_directly(p);
        }
        if let Some(done) = open_launcher(p) {
            return done;
        }
        let missing = on_path(OsStr::new("xdg-open")).is_none();
        let done = missing.then(|| open_without_xdg_open(p)).flatten().unwrap_or_else(|| xdg_open(os(path)));
        if done.is_ok() {
            crate::places::remember_recent(vec![p.to_path_buf()]);
        }
        done
    }

    fn open_ask(&mut self, path: &FsPath) -> Option<OpenAsk> {
        let c = crate::apps::catalog();
        open_question(host(path)?, &c.entries, &c.assoc)
    }

    fn shortcut_folder(&mut self, link: &FsPath) -> Option<FsPath> {
        crate::xdg::fs_path(&launcher_folder(host(link)?)?)
    }

    fn launch(&mut self, l: &Launch) -> Result<(), PlatformError> {
        let done = launch(l);
        if done.is_ok() {
            crate::places::remember_recent(opened_documents(l));
        }
        done
    }

    fn quick_view(&mut self, path: &FsPath) -> bool {
        session_bus() && host(path).is_some_and(|p| sushi_calls(p).iter().any(|args| ask_session(args)))
    }

    fn properties(&mut self, path: &FsPath) -> Vec<flit_core::preview::PropGroup> {
        host(path).map(crate::props::properties).unwrap_or_default()
    }

    fn access(&mut self, items: &[FsPath]) -> Option<(Vec<Option<flit_core::provider::Access>>, flit_core::provider::Accounts)> {
        Some(crate::props::access(items))
    }

    fn needs_exec_bit(&mut self, path: &FsPath) -> bool {
        needs_exec_bit(Path::new(os(path)))
    }

    fn mount(&mut self, device: &FsPath) -> Result<FsPath, PlatformError> {
        if let Some(uri) = crate::places::device_uri(Path::new(os(device)), &crate::places::gvfs_root()) {
            return self.connect(&uri);
        }
        let local = Path::new(os(device));
        if std::fs::metadata(local).is_ok_and(|m| m.is_file()) {
            let point = mount_image(local).map_err(|e| PlatformError::new(-1, format!("cannot mount {}: {e}", device.display())))?;
            return crate::xdg::fs_path(&point).ok_or_else(|| PlatformError::new(-1, "the mount point is not a valid path"));
        }
        let point = crate::udisks::mount(Path::new(os(device))).map_err(|e| crate::places::drive_error(S::ErrorsFsMountFailed, Path::new(os(device)), &e))?;
        crate::xdg::fs_path(&point).ok_or_else(|| PlatformError::new(-1, "the mount point is not a valid path"))
    }

    fn unlock(&mut self, device: &FsPath, passphrase: &str) -> Result<FsPath, PlatformError> {
        let point = crate::udisks::unlock(Path::new(os(device)), passphrase).map_err(|e| crate::places::drive_error(S::ErrorsFsUnlockFailed, Path::new(os(device)), &e))?;
        crate::xdg::fs_path(&point).ok_or_else(|| PlatformError::new(-1, "the mount point is not a valid path"))
    }

    fn connect(&mut self, uri: &str) -> Result<FsPath, PlatformError> {
        let point = crate::places::connect(uri).map_err(|e| crate::places::drive_error(S::ErrorsFsConnectFailed, Path::new(uri), &e))?;
        crate::xdg::fs_path(&point).ok_or_else(|| PlatformError::new(-1, "the mount point is not a valid path"))
    }
}

impl FileOps for LinuxShell {
    fn file_op(&mut self, op: &FileOp, job: &Job) -> OpOutcome {
        let free_space = |p: &Path| p.ancestors().find_map(flit_platform_linux::mounts::space).map(|s| s.1);
        let env = flit_archive::Env { utc_offset: <crate::Linux as flit_platform::Platform>::utc_offset_minutes(), free_space: &free_space };
        if let Some(out) = flit_archive::file_op(op, job, &env) {
            fs::rescan_blind();
            return out;
        }
        let trash = places::trash();
        let out = match (op, &trash) {
            (FileOp::InstallFonts(fonts), _) => install_fonts(fonts, crate::xdg::data_home().map(|d| d.join("fonts"))),
            (FileOp::Purge(pairs), Ok(t)) => places::purge(t, pairs),
            (FileOp::EmptyBin, Ok(t)) => places::empty(t, &places::storage_tops()),
            (FileOp::Purge(_) | FileOp::EmptyBin, Err(e)) => OpOutcome { items: Vec::new(), error: Some(e.clone()) },
            (FileOp::SetAccess { .. } | FileOp::Access(_), _) => crate::elevated::access_op(op, job, || run_op(op, job, None)),
            _ => crate::elevated::file_op(op, job, || run_op(op, job, trash.as_ref().ok())),
        };
        fs::rescan_blind();
        out
    }

    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        let own = OWN_FILES.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let found = crate::window::clipboard_types().and_then(|t| crate::clipboard::files(&t)).or_else(helper_files);
        let (paths, cut) = match found {
            Some((paths, cut)) => {
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

const POSTER_TIMEOUT: Duration = Duration::from_secs(3);

pub fn poster_command(path: &Path, size: u32) -> Option<Vec<OsString>> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let media = crate::mime::mime_of(ext.as_bytes()).unwrap_or_default();
    let px = size.to_string();
    let file = path.as_os_str().to_owned();
    let args: Vec<OsString> = if ext == "pdf" {
        ["pdftoppm", "-png", "-f", "1", "-l", "1", "-singlefile", "-scale-to", &px].into_iter().map(OsString::from).chain([file, "-".into()]).collect()
    } else if media.starts_with("video/") || media.starts_with("audio/") {
        ["ffmpegthumbnailer", "-c", "png", "-o", "-", "-s", &px, "-i"].into_iter().map(OsString::from).chain([file]).collect()
    } else {
        return None;
    };
    Some(args)
}

fn poster(path: &Path, size: u32) -> Option<Bitmap> {
    if let Some(b) = crate::thumb::thumbnail(path, size, None) {
        return Some(b);
    }
    let args = poster_command(path, size)?;
    let mut cmd = Command::new(on_path(&args[0])?);
    let child = cmd.args(&args[1..]).stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped()).spawn().ok()?;
    let png = wait_output(child, POSTER_TIMEOUT)?;
    crate::thumb::png_bytes(&png).map(|b| crate::thumb::fit(b, size))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    File,
    Folder,
    Missing,
}

pub fn link_target_kind(path: &FsPath) -> Option<LinkTarget> {
    let p = host(path)?;
    if !std::fs::symlink_metadata(p).ok()?.file_type().is_symlink() {
        return None;
    }
    Some(match std::fs::metadata(p) {
        Ok(m) if m.is_dir() => LinkTarget::Folder,
        Ok(_) => LinkTarget::File,
        Err(_) => LinkTarget::Missing,
    })
}

pub fn link_icon_names(req: &IconRequest, target: LinkTarget) -> Vec<String> {
    match target {
        LinkTarget::Folder => vec!["folder".into()],
        LinkTarget::Missing => vec!["image-missing".into(), "text-x-generic".into(), "unknown".into()],
        LinkTarget::File => icon_names(&IconRequest { kind: IconKind::Type, ..req.clone() }),
    }
}

fn link_icon(req: &IconRequest, target: LinkTarget) -> Option<Bitmap> {
    let base = crate::icons::icon(&link_icon_names(req, target), req.size)?;
    let side = req.size / 2;
    match (side >= 8).then(|| crate::icons::icon(&["emblem-symbolic-link".to_owned()], side)).flatten() {
        Some(emblem) => Some(crate::icons::with_emblem(base, &emblem)),
        None => Some(base),
    }
}

pub fn icon_names(req: &IconRequest) -> Vec<String> {
    let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect();
    let path = req.path.as_bytes();
    match req.kind {
        IconKind::Folder | IconKind::CustomFolder => owned(&["folder"]),
        IconKind::Place => owned(&[place_icon_name(&req.path), "folder"]),
        IconKind::Volume if path.starts_with(b"/media/") || path.starts_with(b"/run/media/") => owned(&["drive-removable-media", "drive-harddisk"]),
        IconKind::Volume => owned(&["drive-harddisk"]),
        IconKind::Type | IconKind::File | IconKind::Thumbnail => {
            let name = req.path.file_name();
            let ext = name.iter().rposition(|&b| b == b'.').filter(|&i| i > 0).map(|i| &name[i + 1..]);
            let mut names = ext.and_then(crate::mime::mime_of).map(|m| crate::mime::icon_names(&m)).unwrap_or_default();
            names.extend(owned(&["text-x-generic", "unknown"]));
            names
        }
    }
}

fn place_icon_name(path: &FsPath) -> &'static str {
    let local = Path::new(OsStr::from_bytes(path.as_bytes()));
    match path.kind() {
        PathKind::RecycleBin => "user-trash",
        PathKind::Home => "user-home",
        PathKind::Computer => "computer",
        PathKind::Shell => "network-workgroup",
        _ if crate::xdg::home().as_deref() == Some(local) => "user-home",
        _ => crate::xdg::user_dir_icon(local).unwrap_or("folder"),
    }
}

static OWN_FILES:std::sync::Mutex<Option<(String, bool)>> = std::sync::Mutex::new(None);

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

fn helper_files() -> Option<(Vec<FsPath>, bool)> {
    use crate::clipboard::{files, GNOME, KDE_CUT, URI_LIST};
    let read = |mime: &str| clipboard_bytes(mime).filter(|b| !b.is_empty()).map(|b| (mime.to_owned(), b));
    if let Some(found) = read(GNOME).and_then(|g| files(&[g])) {
        return Some(found);
    }
    let uris = read(URI_LIST)?;
    files(&[uris].into_iter().chain(read(KDE_CUT)).collect::<Vec<_>>())
}

pub fn helper_offer(paths: &[FsPath], cut: bool) -> (&'static str, Vec<u8>) {
    if cut {
        (crate::clipboard::GNOME, crate::clipboard::gnome(paths, cut))
    } else {
        (crate::clipboard::URI_LIST, crate::clipboard::uri_list(paths))
    }
}

pub fn set_clipboard_files(paths: &[FsPath], cut: bool) -> bool {
    let (mime, list) = helper_offer(paths, cut);
    offer_through_helper(mime, &list)
}

pub fn set_clipboard_image(png: &[u8]) -> bool {
    offer_through_helper(crate::clipboard::PNG, png)
}

fn offer_through_helper(mime: &str, bytes: &[u8]) -> bool {
    let args: &[&str] = if wayland() { &["wl-copy", "--type", mime] } else { &["xclip", "-selection", "clipboard", "-target", mime, "-in"] };
    let Some(mut cmd) = helper(args) else { return false };
    let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).process_group(0).spawn() else { return false };
    let written = child.stdin.take().is_some_and(|mut input| input.write_all(bytes).is_ok());
    let _ = std::thread::Builder::new().name("flit-reap".into()).spawn(move || {
        let _ = child.wait();
    });
    written
}

pub(crate) fn wait_output(mut child: Child, limit: Duration) -> Option<Vec<u8>> {
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

pub fn launcher_progress(percent: Option<u32>) {
    let dict = match percent {
        Some(v) => format!("{{'progress': <{:.2}>, 'progress-visible': <true>}}", f64::from(v.min(100)) / 100.0),
        None => "{'progress': <0.0>, 'progress-visible': <false>}".to_owned(),
    };
    let args = vec![
        "gdbus".to_owned(),
        "emit".to_owned(),
        "--session".to_owned(),
        "--object-path".to_owned(),
        format!("/com/canonical/unity/launcherentry/{}", std::process::id()),
        "--signal".to_owned(),
        "com.canonical.Unity.LauncherEntry.Update".to_owned(),
        "application://flit.desktop".to_owned(),
        dict,
    ];
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let Some(mut cmd) = helper(&refs) else { return };
    let Ok(mut child) = cmd.spawn() else { return };
    let _ = std::thread::Builder::new().name("flit-reap".into()).spawn(move || {
        let _ = child.wait();
    });
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
    scheme_dark(n.into())
}

fn gtk_theme_dark() -> Option<bool> {
    std::env::var("GTK_THEME").ok().map(|t| t.to_ascii_lowercase().ends_with(":dark") || t.to_ascii_lowercase().contains("-dark"))
}

fn session_bus() -> bool {
    std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() || crate::xdg::runtime_dir().is_some_and(|d| d.join("bus").exists())
}

const SUSHI_WAIT: Duration = Duration::from_secs(4);

pub fn sushi_calls(path: &Path) -> [Vec<String>; 2] {
    let uri = format!("file://{}", flit_platform_linux::percent_encode(path.as_os_str().as_bytes()));
    let call = |interface: &str, window: &str| {
        let method = format!("{interface}.ShowFile");
        ["gdbus", "call", "--session", "--timeout", "3", "--dest", "org.gnome.NautilusPreviewer", "--object-path", "/org/gnome/NautilusPreviewer", "--method", &method, &uri, window, "true"].map(str::to_owned).to_vec()
    };
    [call("org.gnome.NautilusPreviewer2", "''"), call("org.gnome.NautilusPreviewer", "0")]
}

fn ask_session(args: &[String]) -> bool {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    helper(&refs).and_then(|mut c| c.stdout(Stdio::piped()).spawn().ok()).and_then(|child| wait_output(child, SUSHI_WAIT)).is_some()
}

pub fn gsettings(args: &[&str]) -> Option<String> {
    let full: Vec<&str> = ["gsettings", "get"].into_iter().chain(args.iter().copied()).collect();
    let child = helper(&full)?.stdout(Stdio::piped()).spawn().ok()?;
    String::from_utf8(wait_output(child, HELPER_TIMEOUT)?).ok()
}

fn probe_dark() -> Option<bool> {
    if let Some(dark) = gtk_theme_dark() {
        return Some(dark);
    }
    let portal = ["gdbus", "call", "--session", "--timeout", "1", "--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop", "--method", "org.freedesktop.portal.Settings.Read", "org.freedesktop.appearance", "color-scheme"];
    let gsettings = ["gsettings", "get", "org.gnome.desktop.interface", "color-scheme"];
    let ask = |args: &[&str]| helper(args).and_then(|mut c| c.stdout(Stdio::piped()).spawn().ok()).and_then(|child| wait_output(child, HELPER_TIMEOUT)).and_then(|r| parse_color_scheme(&r));
    let kde = || crate::xdg::config_home().and_then(|c| std::fs::read_to_string(c.join("kdeglobals")).ok()).and_then(|t| kde_dark(&t));
    session_bus().then(|| ask(&portal)).flatten().or_else(kde).or_else(|| ask(&gsettings))
}

fn parse_flag(out: &str) -> Option<bool> {
    let v = out.trim();
    match (v.contains("true"), v.contains("false")) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

pub fn probe_animations() -> Option<bool> {
    let portal = ["gdbus", "call", "--session", "--timeout", "1", "--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop", "--method", "org.freedesktop.portal.Settings.Read", "org.gnome.desktop.interface", "enable-animations"];
    let gsettings = ["gsettings", "get", "org.gnome.desktop.interface", "enable-animations"];
    let ask = |args: &[&str]| helper(args).and_then(|mut c| c.stdout(Stdio::piped()).spawn().ok()).and_then(|child| wait_output(child, HELPER_TIMEOUT)).and_then(|r| String::from_utf8(r).ok()).and_then(|r| parse_flag(&r));
    session_bus().then(|| ask(&portal)).flatten().or_else(|| ask(&gsettings))
}

pub fn last_number(out: &str) -> Option<u32> {
    let end = out.rfind(|c: char| c.is_ascii_digit())? + 1;
    let start = out[..end].rfind(|c: char| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    out[start..end].parse().ok()
}

pub fn probe_double_click() -> Option<u32> {
    let portal = ["gdbus", "call", "--session", "--timeout", "1", "--dest", "org.freedesktop.portal.Desktop", "--object-path", "/org/freedesktop/portal/desktop", "--method", "org.freedesktop.portal.Settings.Read", "org.gnome.desktop.peripherals.mouse", "double-click"];
    let gsettings = ["gsettings", "get", "org.gnome.desktop.peripherals.mouse", "double-click"];
    let ask = |args: &[&str]| helper(args).and_then(|mut c| c.stdout(Stdio::piped()).spawn().ok()).and_then(|child| wait_output(child, HELPER_TIMEOUT)).and_then(|r| String::from_utf8(r).ok()).and_then(|r| last_number(&r));
    session_bus().then(|| ask(&portal)).flatten().or_else(|| ask(&gsettings))
}

pub fn play_media(video: bool, path: &Path, socket: &Path, volume: Option<f32>) -> Option<(std::process::Child, bool)> {
    let file = path.as_os_str().to_owned();
    let command = |name: &str, extra: &[&str]| {
        on_path(OsStr::new(name)).map(|program| {
            let mut cmd = Command::new(program);
            cmd.args(extra.iter().map(OsStr::new)).arg(&file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
            cmd
        })
    };
    let mpv = mpv_args(socket, volume);
    let mpv: Vec<&str> = mpv.iter().map(String::as_str).collect();
    if let Some(mut with) = command("mpv", &mpv) {
        let _ = std::fs::remove_file(socket);
        return with.spawn().ok().map(|c| (c, true));
    }
    let mut with = command("gst-play-1.0", &[])
        .or_else(|| if video { command("ffplay", &["-autoexit"]) } else { command("ffplay", &["-autoexit", "-nodisp"]) })
        .or_else(|| if video { None } else { command("paplay", &[]) })?;
    with.spawn().ok().map(|c| (c, false))
}

pub fn mpv_args(socket: &Path, volume: Option<f32>) -> Vec<String> {
    let mut args = vec!["--no-terminal".to_owned(), format!("--input-ipc-server={}", socket.display())];
    args.extend(volume.map(|v| format!("--volume={}", (v.clamp(0.0, 1.0) * 100.0).round())));
    args
}

pub fn kde_dark(kdeglobals: &str) -> Option<bool> {
    let mut window = false;
    for line in kdeglobals.lines().map(str::trim) {
        if line.starts_with('[') {
            window = line == "[Colors:Window]";
        } else if let Some(rgb) = line.strip_prefix("BackgroundNormal=").filter(|_| window) {
            let c: Vec<u32> = rgb.split(',').filter_map(|v| v.trim().parse().ok()).collect();
            if let [r, g, b, ..] = c[..] {
                return Some(r * 299 + g * 587 + b * 114 < 128_000);
            }
        }
    }
    None
}

pub fn scheme_dark(scheme: i64) -> Option<bool> {
    match scheme {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

pub fn dark_preference(found: impl Fn(bool) + Send + 'static) {
    let _ = std::thread::Builder::new().name("flit-theme".into()).spawn(move || {
        if let Some(dark) = probe_dark() {
            found(dark);
        }
        if let Some(address) = crate::dbus::session_address() {
            crate::atspi::watch_color_scheme(&address, &|scheme| {
                if let Some(dark) = scheme_dark(scheme).or_else(probe_dark) {
                    found(dark);
                }
            });
        }
    });
}

#[cfg(test)]
#[path = "../test/shell.rs"]
mod tests;
