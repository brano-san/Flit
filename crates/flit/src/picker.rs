use crate::{app, wtf8, P};
use flit_core::i18n::{self, Lang, S};
use flit_core::{FsPath, Location, Open};
use flit_core::pick::{self, Request};
use flit_platform::{FilePlatform, PlatformError, ShellPlatform, SystemPlatform};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub type Picked = (Vec<FsPath>, usize, pick::Choices);
pub type ResultSlot = Arc<Mutex<Option<Picked>>>;

pub(crate) fn language() {
    let lang = Lang::from_code(&std::env::var("FLIT_LANG").unwrap_or_else(|_| P::ui_language())).unwrap_or_default();
    i18n::set_system_lang(lang);
    i18n::set_lang(lang);
}

fn initial(request: &Request) -> Open {
    let unix = P::caps().unix_paths;
    let cwd = std::env::current_dir().ok().map(|p| wtf8(p.as_os_str()));
    let mut open = Open { cwd, ..Default::default() };
    let path = request.path.as_deref().and_then(|raw| flit_core::open::resolve_for(raw, open.cwd.as_deref(), &P::known_folder, unix).ok());
    match path.and_then(|loc| pick::local_path(&loc).cloned()) {
        Some(path) if flit_platform::std_path(&path).is_dir() => open.paths.push(Location::from_local(path).to_bytes()),
        Some(path) if request.directories() => {
            open.paths.push(Location::from_local(path.parent().unwrap_or_else(P::home)).to_bytes());
        }
        Some(path) => open.select.push(Location::from_local(path).to_bytes()),
        None => open.paths.push(Location::from_local(P::home()).to_bytes()),
    }
    open
}

fn choose(request: Request, started: Instant) -> Result<Option<Picked>, PlatformError> {
    let open = initial(&request);
    let output: ResultSlot = Arc::default();
    app::run::<P>(open, None, None, false, false, Some((request, output.clone())), started)?;
    let picked = output.lock().unwrap_or_else(|e| e.into_inner()).take();
    Ok(picked)
}

pub fn cli(request: Option<Request>, started: Instant) -> i32 {
    #[cfg(windows)]
    flit_platform_windows::diagnose::attach_console();
    language();
    let Some(request) = request else {
        eprintln!("{}", S::PickUsage.t());
        return 2;
    };
    match choose(request, started) {
        Ok(Some((paths, _, _))) => {
            let result = write_paths(&paths);
            if result.is_ok() { 0 } else { 2 }
        }
        Ok(None) => 1,
        Err(e) => {
            eprintln!("{}", flit_core::tr!(S::UiAppStartFailed, error = i18n::localize(&e.to_string())));
            2
        }
    }
}

#[cfg(target_os = "linux")]
pub fn portal_request(started: Instant) -> i32 {
    use std::io::Read;
    language();
    let mut input = Vec::new();
    if io::stdin().take(1 << 20).read_to_end(&mut input).is_err() {
        return 2;
    }
    let Ok(request) = serde_json::from_slice(&input) else { return 2 };
    match choose(request, started) {
        Ok(Some((paths, filter, choices))) => {
            let paths: Vec<_> = paths.iter().map(|p| p.as_bytes()).collect();
            if serde_json::to_writer(io::stdout().lock(), &(paths, filter, choices)).is_ok() { 0 } else { 2 }
        }
        Ok(None) => 1,
        Err(e) => {
            tracing::warn!("{e}");
            2
        }
    }
}

fn write_paths(paths: &[FsPath]) -> io::Result<()> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::{GetConsoleMode, GetStdHandle, WriteConsoleW, CONSOLE_MODE, STD_OUTPUT_HANDLE};

        let mut mode = CONSOLE_MODE::default();
        let console = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.ok()
            .filter(|handle| unsafe { GetConsoleMode(*handle, &mut mode) }.is_ok());
        if let Some(handle) = console {
            return write_console_paths(paths, |units| {
                let mut written = 0;
                unsafe { WriteConsoleW(handle, units, Some(&mut written), None) }
                    .map_err(io::Error::other)?;
                Ok(written as usize)
            });
        }
    }
    write_redirected_paths(paths, io::stdout().lock())
}

fn write_redirected_paths(paths: &[FsPath], mut out: impl Write) -> io::Result<()> {
    for path in paths {
        #[cfg(windows)]
        let text = String::from_utf8_lossy(path.as_bytes());
        #[cfg(windows)]
        out.write_all(text.as_bytes())?;
        #[cfg(not(windows))]
        out.write_all(path.as_bytes())?;
        out.write_all(b"\n")?;
    }
    out.flush()
}

#[cfg(windows)]
fn write_console_paths(paths: &[FsPath], mut write: impl FnMut(&[u16]) -> io::Result<usize>) -> io::Result<()> {
    for path in paths {
        let mut line = path.to_wide(false);
        line.pop();
        line.extend([13, 10]);
        let mut remaining = line.as_slice();
        while !remaining.is_empty() {
            let written = write(remaining)?;
            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            remaining = &remaining[written..];
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../test/picker.rs"]
mod tests;
