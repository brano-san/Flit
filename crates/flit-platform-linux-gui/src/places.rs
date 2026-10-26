use flit_core::listing::{attr, Meta};
use flit_core::ui::Drive;
use flit_core::{FsError, FsErrorKind, ListingBuilder};
use flit_platform::{OpOutcome, PlatformError};
use flit_platform_linux::mounts::{self, Mount};
use flit_platform_linux::Trash;
use std::os::unix::ffi::OsStrExt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Volume {
    pub mount: Mount,
    pub label: String,
    pub total: u64,
    pub free: u64,
}

fn name_of(m: &Mount) -> String {
    match m.point.as_slice() {
        b"/" => "File System".into(),
        p => String::from_utf8_lossy(p.rsplit(|&b| b == b'/').next().unwrap_or(p)).into_owned(),
    }
}

pub fn volumes_from(table: &[Mount], user: &[u8], by_label: &Path) -> Vec<Volume> {
    let labels = mounts::labels(by_label);
    mounts::visible(table, user)
        .into_iter()
        .map(|m| {
            let (total, free) = mounts::space(m.path()).unwrap_or_default();
            let label = mounts::label_of(&m, &labels).filter(|_| m.point != b"/").unwrap_or_else(|| name_of(&m));
            Volume { mount: m, label, total, free }
        })
        .collect()
}

pub fn volumes() -> Vec<Volume> {
    volumes_from(&mounts::read().unwrap_or_default(), &mounts::user_name(), Path::new(mounts::BY_LABEL))
}

pub fn drives() -> Vec<Drive> {
    volumes()
        .into_iter()
        .filter_map(|v| Some(Drive { path: crate::xdg::fs_path(v.mount.path())?, label: v.label, total: v.total, free: v.free, removable: v.mount.removable(), unmounted: false }))
        .chain(unmounted())
        .collect()
}

fn unmounted() -> Vec<Drive> {
    let out = std::process::Command::new("lsblk").args(["-P", "-b", "-o", "PATH,LABEL,MOUNTPOINT,HOTPLUG,FSTYPE"]).stderr(std::process::Stdio::null()).output();
    out.map(|o| unmounted_from(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

pub fn unmounted_from(lsblk: &str) -> Vec<Drive> {
    let mut out = Vec::new();
    for line in lsblk.lines() {
        let mut f = HashMap::new();
        let mut rest = line.trim();
        while let Some((key, tail)) = rest.split_once("=\"") {
            let Some((value, after)) = tail.split_once('"') else { break };
            f.insert(key.trim(), value);
            rest = after;
        }
        let get = |k: &str| f.get(k).copied().unwrap_or("");
        if get("MOUNTPOINT").is_empty() && get("HOTPLUG") == "1" && !matches!(get("FSTYPE"), "" | "swap" | "crypto_LUKS") {
            let Some(path) = flit_core::FsPath::parse_exact(get("PATH").as_bytes()) else { continue };
            let label = if get("LABEL").is_empty() { get("PATH").rsplit('/').next().unwrap_or("").to_owned() } else { get("LABEL").to_owned() };
            out.push(Drive { path, label, total: 0, free: 0, removable: true, unmounted: true });
        }
    }
    out
}

pub fn mount(device: &Path) -> std::io::Result<PathBuf> {
    let out = std::process::Command::new("udisksctl").args(["mount", "--no-user-interaction", "-b"]).arg(device).output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    let table = mounts::read()?;
    let found = table.iter().rev().find(|m| m.source == device.as_os_str().as_bytes()).map(|m| m.path().to_path_buf());
    found.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "mounted, but the mount point is unknown"))
}

pub fn eject_steps(table: &[Mount], point: &Path) -> Option<[[std::ffi::OsString; 4]; 2]> {
    let m = table.iter().rev().find(|m| m.path() == point && m.source.starts_with(b"/dev/"))?;
    let dev = std::ffi::OsStr::from_bytes(&m.source).to_owned();
    let step = |verb: &str| ["udisksctl".into(), verb.into(), "-b".into(), dev.clone()];
    Some([step("unmount"), step("power-off")])
}

pub fn eject(point: &Path) -> std::io::Result<()> {
    let table = mounts::read()?;
    let steps = eject_steps(&table, point).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "not a mounted block device"))?;
    std::thread::Builder::new().name("flit-eject".into()).spawn(move || {
        for [program, args @ ..] in steps {
            match std::process::Command::new(&program).args(&args).status() {
                Ok(s) if s.success() => {}
                Ok(s) => return tracing::warn!("{} {:?} failed: {s}", program.to_string_lossy(), args),
                Err(e) => return tracing::warn!("cannot run udisksctl: {e}"),
            }
        }
    })?;
    Ok(())
}

pub fn list_computer(out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let table = mounts::read().map_err(|e| FsError { kind: FsErrorKind::Unavailable, code: e.raw_os_error().unwrap_or(-1) as i64 })?;
    for m in mounts::visible(&table, &mounts::user_name()) {
        let mut meta = Meta { attrs: attr::DIR | attr::VOLUME, ..Default::default() };
        if let Some((total, _)) = mounts::space(m.path()) {
            meta.size = total;
            meta.attrs |= attr::SIZE_KNOWN;
        }
        out.push(&m.point, meta);
    }
    out.batch_done();
    Ok(())
}

pub fn watch_mounts(changed: Box<dyn Fn() + Send>) {
    if let Err(e) = mounts::watch(Path::new(mounts::MOUNTINFO), changed) {
        eprintln!("flit: cannot watch {}: {e}", mounts::MOUNTINFO);
    }
}

pub fn storage_tops() -> Vec<PathBuf> {
    mounts::read().unwrap_or_default().iter().filter(|m| m.is_storage()).map(|m| m.path().to_path_buf()).collect()
}

pub fn trash() -> Result<Trash, PlatformError> {
    Trash::from_env().ok_or_else(|| PlatformError::new(-1, "The Trash folder is unknown: neither XDG_DATA_HOME nor HOME is set"))
}

pub fn list_trash(out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let trash = trash().map_err(|_| FsError { kind: FsErrorKind::Unavailable, code: 0 })?;
    trash.list(&trash.dirs(&storage_tops()), out);
    Ok(())
}

fn failure(p: &Path, e: std::io::Error) -> PlatformError {
    PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("{}: {e}", p.display()))
}

pub fn purge(trash: &Trash, pairs: &[(flit_core::FsPath, flit_core::FsPath)]) -> OpOutcome {
    let mut out = OpOutcome::default();
    for (trashed, original) in pairs {
        let Some(path) = flit_platform_linux::host(trashed) else {
            out.error = Some(PlatformError::new(-1, format!("{} is not in the Trash", trashed.display())));
            continue;
        };
        match trash.purge(path) {
            Ok(()) => out.items.push((trashed.clone(), original.clone())),
            Err(e) => out.error = Some(failure(path, e)),
        }
    }
    out
}

pub fn empty(trash: &Trash, tops: &[PathBuf]) -> OpOutcome {
    let dirs = trash.dirs(tops);
    let (removed, error) = trash.empty(&dirs);
    if removed.is_empty() && error.is_none() {
        return OpOutcome { items: Vec::new(), error: Some(PlatformError::new(-1, "The Trash is already empty")) };
    }
    let items = removed.iter().filter_map(|p| crate::xdg::fs_path(p)).map(|p| (p.clone(), p)).collect();
    OpOutcome { items, error: error.map(|e| PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("Some items could not be deleted from the Trash: {e}"))) }
}

fn attribute<'a>(tag: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let key = [b" ", name, b"=\""].concat();
    let at = tag.windows(key.len()).position(|w| w == key.as_slice())? + key.len();
    let len = tag[at..].iter().position(|&b| b == b'"')?;
    Some(&tag[at..at + len])
}

fn unescape_xml(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let entity = [(&b"&amp;"[..], b'&'), (b"&lt;", b'<'), (b"&gt;", b'>'), (b"&quot;", b'"'), (b"&apos;", b'\'')].into_iter().find(|(e, _)| s[i..].starts_with(e));
        match entity {
            Some((e, c)) => {
                out.push(c);
                i += e.len();
            }
            None => {
                out.push(s[i]);
                i += 1;
            }
        }
    }
    out
}

fn utc_ticks(s: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(s.get(..19)?).ok()?;
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i32>().ok();
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = num(0..4)? - 1900;
    tm.tm_mon = num(5..7)? - 1;
    tm.tm_mday = num(8..10)?;
    tm.tm_hour = num(11..13)?;
    tm.tm_min = num(14..16)?;
    tm.tm_sec = num(17..19)?;
    let t = unsafe { libc::timegm(&mut tm) };
    (t != -1).then(|| t * 10_000_000 + 116_444_736_000_000_000)
}

pub fn bookmarks_from(gtk: &[u8], kde: &[u8], skip: &[&[u8]], is_dir: impl Fn(&Path) -> bool) -> Vec<flit_core::Location> {
    let gtk = gtk.split(|&b| b == b'\n').filter_map(|l| l.split(|&b| b == b' ').next());
    let kde = kde.split(|&b| b == b'<').filter(|t| t.starts_with(b"bookmark ")).filter_map(|t| attribute(t, b"href")).map(unescape_xml);
    let mut out: Vec<flit_core::Location> = Vec::new();
    for uri in gtk.map(<[u8]>::to_vec).chain(kde) {
        let Some(path) = crate::instance::decode_uri(&uri).filter(|p| p.starts_with(b"/") && !skip.contains(&p.as_slice())) else { continue };
        let Some(p) = flit_core::FsPath::parse_exact(&path).filter(|_| is_dir(Path::new(std::ffi::OsStr::from_bytes(&path)))) else { continue };
        let loc = flit_core::Location::from_local(p);
        if !out.contains(&loc) {
            out.push(loc);
        }
    }
    out
}

pub fn bookmarks() -> Vec<flit_core::Location> {
    let read = |p: Option<PathBuf>| p.and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    let home = std::env::var_os("HOME").unwrap_or_default();
    let gtk = read(crate::xdg::config_home().map(|c| c.join("gtk-3.0/bookmarks")));
    let kde = read(crate::xdg::data_home().map(|d| d.join("user-places.xbel")));
    bookmarks_from(&gtk, &kde, &[home.as_bytes(), b"/"], Path::is_dir)
}

pub fn recent_from(xbel: &[u8], limit: usize, exists: impl Fn(&Path) -> Option<bool>) -> Vec<flit_core::ui::Recent> {
    let mut found: Vec<(i64, Vec<u8>)> = xbel
        .split(|&b| b == b'<')
        .filter(|t| t.starts_with(b"bookmark "))
        .filter_map(|tag| {
            let href = unescape_xml(attribute(tag, b"href")?);
            let path = crate::instance::decode_uri(&href).filter(|p| p.starts_with(b"/"))?;
            let at = attribute(tag, b"modified").or_else(|| attribute(tag, b"visited")).and_then(utc_ticks).unwrap_or(0);
            Some((at, path))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut out: Vec<flit_core::ui::Recent> = Vec::new();
    for (at, path) in found {
        if out.len() == limit {
            break;
        }
        let Some(target) = flit_core::FsPath::parse_exact(&path) else { continue };
        if out.iter().any(|r| r.target == target) {
            continue;
        }
        let Some(dir) = exists(Path::new(std::ffi::OsStr::from_bytes(&path))) else { continue };
        let name = flit_core::name::display(target.file_name()).into_owned();
        out.push(flit_core::ui::Recent { name, target, dir, at });
    }
    out
}

pub fn recent(limit: usize) -> (Vec<flit_core::ui::Recent>, bool) {
    let file = crate::xdg::data_home().map(|d| d.join("recently-used.xbel"));
    let bytes = file.and_then(|f| std::fs::read(f).ok()).unwrap_or_default();
    (recent_from(&bytes, limit, |p| std::fs::metadata(p).ok().map(|m| m.is_dir())), false)
}

#[cfg(test)]
#[path = "../test/places.rs"]
mod tests;
