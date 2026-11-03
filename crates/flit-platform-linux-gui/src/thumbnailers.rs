use flit_core::ui::Bitmap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
pub struct Thumbnailer {
    pub exec: String,
    pub mimes: Vec<String>,
}

pub fn parse(text: &str) -> Option<Thumbnailer> {
    let (mut inside, mut exec, mut mimes, mut try_exec) = (false, None, Vec::new(), None);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Thumbnailer Entry]";
            continue;
        }
        let Some((k, v)) = line.split_once('=').filter(|_| inside).map(|(k, v)| (k.trim(), v.trim())) else { continue };
        match k {
            "Exec" => exec = Some(v.to_owned()),
            "TryExec" => try_exec = Some(v.to_owned()),
            "MimeType" => mimes = v.split(';').filter(|m| !m.is_empty()).map(str::to_owned).collect(),
            _ => {}
        }
    }
    let exec = exec.filter(|e| !e.is_empty())?;
    let program = try_exec.unwrap_or_else(|| exec.split_whitespace().next().unwrap_or_default().to_owned());
    crate::shell::on_path(std::ffi::OsStr::new(&program))?;
    Some(Thumbnailer { exec, mimes })
}

pub fn load(dirs: &[PathBuf]) -> Vec<Thumbnailer> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(read) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "thumbnailer")).collect();
        files.sort();
        for f in files {
            if seen.insert(f.file_name().map(|n| n.to_owned())) {
                out.extend(std::fs::read_to_string(&f).ok().as_deref().and_then(parse));
            }
        }
    }
    out
}

fn installed() -> &'static [Thumbnailer] {
    static ALL: OnceLock<Vec<Thumbnailer>> = OnceLock::new();
    ALL.get_or_init(|| load(&crate::apps::data_dirs().iter().map(|d| d.join("thumbnailers")).collect::<Vec<_>>()))
}

pub fn command(exec: &str, input: &Path, output: &Path, size: u32) -> Vec<OsString> {
    let uri = format!("file://{}", flit_platform_linux::percent_encode(input.as_os_str().as_bytes()));
    exec.split_whitespace()
        .map(|arg| {
            let mut out = Vec::new();
            let mut bytes = arg.as_bytes().iter().copied();
            while let Some(b) = bytes.next() {
                if b != b'%' {
                    out.push(b);
                    continue;
                }
                match bytes.next() {
                    Some(b'i') => out.extend_from_slice(input.as_os_str().as_bytes()),
                    Some(b'u') => out.extend_from_slice(uri.as_bytes()),
                    Some(b'o') => out.extend_from_slice(output.as_os_str().as_bytes()),
                    Some(b's') => out.extend_from_slice(size.to_string().as_bytes()),
                    Some(b'%') => out.push(b'%'),
                    _ => {}
                }
            }
            std::os::unix::ffi::OsStringExt::from_vec(out)
        })
        .collect()
}

pub fn pick<'a>(all: &'a [Thumbnailer], mime: &str) -> Option<&'a Thumbnailer> {
    all.iter().find(|t| t.mimes.iter().any(|m| m.eq_ignore_ascii_case(mime)))
}

pub fn run(t: &Thumbnailer, path: &Path, size: u32) -> Option<Bitmap> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let out = std::env::temp_dir().join(format!("flit-thumb-{}-{}.png", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let args = command(&t.exec, path, &out, size);
    let child = Command::new(crate::shell::on_path(&args[0])?).args(&args[1..]).stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped()).spawn().ok()?;
    let done = crate::shell::wait_output(child, TIMEOUT).is_some();
    let png = std::fs::read(&out).ok();
    let _ = std::fs::remove_file(&out);
    crate::thumb::png_bytes(&png.filter(|_| done)?).map(|b| crate::thumb::fit(b, size))
}

pub fn thumbnail(path: &Path, size: u32) -> Option<Bitmap> {
    let ext = path.extension()?.as_bytes();
    let mime = crate::mime::mime_of(ext)?;
    run(pick(installed(), &mime)?, path, size)
}

#[cfg(test)]
#[path = "../test/thumbnailers.rs"]
mod tests;
