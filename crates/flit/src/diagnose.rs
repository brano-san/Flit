use flit_platform::Platform;
use std::fmt::Write;
use std::path::{Path, PathBuf};

pub const FILE: &str = "flit-diagnose.txt";
pub const LOG_LINES: usize = 200;

pub fn tail(text: &str, n: usize) -> &str {
    match text.trim_end_matches('\n').rmatch_indices('\n').nth(n.saturating_sub(1)) {
        Some((at, _)) => &text[at + 1..],
        None => text,
    }
}

fn shown(p: Option<PathBuf>) -> String {
    p.map_or_else(|| "none".into(), |p| p.display().to_string())
}

fn files_with(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == ext)).collect();
    found.sort();
    found
}

pub fn report<P: Platform>() -> String {
    let logs = P::state_dir().map(|d| d.join("logs"));
    let mut out = String::new();
    let _ = writeln!(out, "Flit {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    let _ = writeln!(out, "OS: {}", P::os_version());
    let _ = writeln!(out, "UI language: {}", P::ui_language());
    let _ = writeln!(out, "Settings: {}", shown(crate::settings::file::<P>()));
    let _ = writeln!(out, "State: {}", shown(P::state_dir()));
    let _ = writeln!(out, "Cache: {}", shown(P::cache_dir()));
    let _ = writeln!(out, "Logs: {}", shown(logs.clone()));
    #[cfg(windows)]
    {
        use flit_platform_windows::diagnose;
        let _ = writeln!(out, "\nGPU:");
        for g in &diagnose::gpus() {
            let _ = writeln!(out, "  {g}");
        }
        diagnose::load_shell_extensions(&P::home());
        let _ = writeln!(out, "\nModules loaded in process outside the Windows folder (shell extensions among them):");
        for m in &diagnose::third_party(&diagnose::modules()) {
            let _ = writeln!(out, "  {}", m.display());
        }
    }
    if let Some(dir) = &logs {
        let _ = writeln!(out, "\nCrash reports:");
        for d in &files_with(dir, "dmp") {
            let _ = writeln!(out, "  {}", d.display());
        }
        for note in files_with(dir, "txt").iter().filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("flit-crash-"))) {
            let _ = writeln!(out, "  {}: {}", note.display(), std::fs::read_to_string(note).unwrap_or_default().replace('\n', "; "));
        }
        let log = std::fs::read_to_string(dir.join(crate::logs::FILE)).unwrap_or_default();
        let _ = writeln!(out, "\nLast {LOG_LINES} log lines:\n{}", tail(&log, LOG_LINES));
    }
    out
}

pub fn run<P: Platform>() -> std::io::Result<PathBuf> {
    let dir = P::state_dir().map_or_else(std::env::temp_dir, |d| d.join("logs"));
    let path = dir.join(FILE);
    crate::settings::write_atomic(&path, report::<P>().as_bytes())?;
    Ok(path)
}

#[cfg(test)]
#[path = "../test/diagnose.rs"]
mod tests;
