use std::path::{Path, PathBuf};

pub(crate) type Process = (String, Vec<PathBuf>);

pub(crate) fn holders_in(target: &Path, procs: impl IntoIterator<Item = Process>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (name, links) in procs {
        if links.iter().any(|l| l.starts_with(target)) && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn process(dir: &Path) -> Option<Process> {
    let name = std::fs::read_to_string(dir.join("comm")).ok()?.trim_end().to_owned();
    let mut links: Vec<PathBuf> = ["cwd", "exe", "root"].iter().filter_map(|l| std::fs::read_link(dir.join(l)).ok()).collect();
    if let Ok(fds) = std::fs::read_dir(dir.join("fd")) {
        links.extend(fds.flatten().filter_map(|fd| std::fs::read_link(fd.path()).ok()));
    }
    Some((name, links))
}

fn processes(proc_root: &Path) -> impl Iterator<Item = Process> {
    std::fs::read_dir(proc_root).into_iter().flatten().flatten().filter(|e| e.file_name().as_encoded_bytes().iter().all(u8::is_ascii_digit)).filter_map(|e| process(&e.path()))
}

pub(crate) fn holders(target: &Path) -> Vec<String> {
    holders_in(target, processes(Path::new("/proc")))
}

#[cfg(test)]
#[path = "../test/busy.rs"]
mod tests;
