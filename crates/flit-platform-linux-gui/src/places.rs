use flit_core::i18n::S;
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

fn name_of(m: &Mount, l: flit_core::i18n::Lang) -> String {
    match m.point.as_slice() {
        b"/" => flit_core::i18n::S::SidebarFolderFileSystem.in_lang(l).into(),
        p => String::from_utf8_lossy(p.rsplit(|&b| b == b'/').next().unwrap_or(p)).into_owned(),
    }
}

type Probe = fn(&Path) -> Option<(u64, u64)>;

fn spaces_of(ms: &[Mount], wait: std::time::Duration, probe: Probe) -> Vec<Option<(u64, u64)>> {
    let remote: Vec<PathBuf> = ms.iter().filter(|m| m.is_network()).map(|m| m.path().to_path_buf()).collect();
    let mut probed = mounts::spaces_within(&remote, wait, probe).into_iter();
    ms.iter().map(|m| if m.is_network() { probed.next().flatten() } else { probe(m.path()) }).collect()
}

pub fn volumes_from(table: &[Mount], user: &[u8], by_label: &Path) -> Vec<Volume> {
    let labels = mounts::labels(by_label);
    let disks = mounts::disks(table, user);
    let spaces = spaces_of(&disks, mounts::NETWORK_SPACE_WAIT, mounts::space);
    disks
        .into_iter()
        .zip(spaces)
        .map(|(m, space)| {
            let (total, free) = space.unwrap_or_default();
            let label = mounts::label_of(&m, &labels).filter(|_| m.point != b"/").unwrap_or_else(|| name_of(&m, flit_core::i18n::lang()));
            Volume { mount: m, label, total, free }
        })
        .collect()
}

pub fn volumes() -> Vec<Volume> {
    volumes_from(&mounts::read().unwrap_or_default(), &mounts::user_name(), Path::new(mounts::BY_LABEL))
}

pub fn drive_of(v: &Volume) -> Option<Drive> {
    let remote = v.mount.is_network().then(|| String::from_utf8_lossy(&v.mount.source).into_owned());
    Some(Drive { path: crate::xdg::fs_path(v.mount.path())?, label: v.label.clone(), total: v.total, free: v.free, file_system: v.mount.fstype.clone(), removable: v.mount.removable(), unmounted: false, kind: None, remote })
}

pub fn drives() -> Vec<Drive> {
    volumes()
        .iter()
        .filter_map(drive_of)
        .chain(unmounted())
        .chain(gvfs(&gvfs_root()))
        .chain(phones(&gvfs_root()))
        .collect()
}

pub fn gvfs_root() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })), PathBuf::from).join("gvfs")
}

const DEVICE_SCHEMES: [&str; 3] = ["mtp", "gphoto2", "afc"];

pub fn gvfs_uri(name: &str) -> Option<String> {
    let (scheme, rest) = name.split_once(':')?;
    let host = rest.split(',').find_map(|kv| kv.strip_prefix("host="))?;
    DEVICE_SCHEMES.contains(&scheme).then(|| format!("{scheme}://{host}/"))
}

fn gvfs_name(uri: &str) -> Option<String> {
    let (scheme, rest) = uri.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    DEVICE_SCHEMES.contains(&scheme).then(|| format!("{scheme}:host={host}"))
}

pub fn device_uri(path: &Path, root: &Path) -> Option<String> {
    gvfs_uri(path.strip_prefix(root).ok()?.to_str()?)
}

pub fn phones_from(listed: &str, root: &Path) -> Vec<Drive> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let lines: Vec<&str> = listed.lines().collect();
    let mut out: Vec<Drive> = Vec::new();
    for (k, line) in lines.iter().enumerate() {
        let Some(name) = line.trim_start().strip_prefix("Volume(").and_then(|r| r.split_once("): ")).map(|(_, n)| n.trim()) else { continue };
        let depth = indent(line);
        let block = lines[k + 1..].iter().take_while(|l| l.trim().is_empty() || indent(l) > depth);
        let (mut uri, mut mounted) = (None, false);
        for l in block {
            let l = l.trim();
            mounted |= l.starts_with("Mount(");
            uri = uri.or_else(|| l.strip_prefix("activation_root=").map(str::to_owned));
        }
        let Some(dir) = uri.as_deref().filter(|_| !mounted).and_then(gvfs_name) else { continue };
        let Some(path) = crate::xdg::fs_path(&root.join(dir)) else { continue };
        if !out.iter().any(|d| d.path == path) {
            out.push(Drive { path, label: name.to_owned(), total: 0, free: 0, file_system: String::new(), removable: true, unmounted: true, kind: None, remote: None });
        }
    }
    out
}

fn phones(root: &Path) -> Vec<Drive> {
    let listed = gio(&["mount", "-li"], PHONES_WAIT).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    phones_from(&listed, root)
}

pub fn gvfs_label(name: &str) -> String {
    let (kind, rest) = name.split_once(':').unwrap_or(("", name));
    let field = |k: &str| rest.split(',').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='));
    match (field("share"), field("host").or(field("server"))) {
        (Some(share), Some(host)) => format!("{share} ({host})"),
        (None, Some(host)) => format!("{host} ({kind})"),
        _ => name.to_owned(),
    }
}

pub fn gvfs(root: &Path) -> Vec<Drive> {
    let Ok(dir) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<Drive> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = crate::xdg::fs_path(&e.path())?;
            Some(Drive { path, label: gvfs_label(&name), total: 0, free: 0, file_system: String::new(), removable: false, unmounted: false, kind: None, remote: Some(name) })
        })
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

pub fn local_path(gio_info: &str) -> Option<PathBuf> {
    gio_info.lines().find_map(|l| l.trim().strip_prefix("local path:")).map(|p| PathBuf::from(p.trim()))
}

pub const CONNECT_WAIT: std::time::Duration = std::time::Duration::from_secs(45);
const PHONES_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

pub fn run_within(mut cmd: std::process::Command, wait: std::time::Duration) -> std::io::Result<std::process::Output> {
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let deadline = std::time::Instant::now() + wait;
    while child.try_wait()?.is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, S::ErrorsFsNotResponding.en()));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    child.wait_with_output()
}

pub fn needs_login(out: &std::process::Output) -> bool {
    let said = [&out.stdout[..], &out.stderr[..]].concat();
    let text = String::from_utf8_lossy(&said);
    !out.status.success() && ["Authentication Required", "Password:", "Password dialog cancelled"].iter().any(|w| text.contains(w))
}

fn gio(args: &[&str], wait: std::time::Duration) -> std::io::Result<std::process::Output> {
    let mut cmd = crate::tools::command("gio")?;
    cmd.args(args);
    run_within(cmd, wait)
}

pub fn connect(uri: &str) -> std::io::Result<PathBuf> {
    let mounted = gio(&["mount", uri], CONNECT_WAIT)?;
    if needs_login(&mounted) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, S::ErrorsFsNeedsLogin.en()));
    }
    let info = gio(&["info", uri], CONNECT_WAIT)?;
    local_path(&String::from_utf8_lossy(&info.stdout)).ok_or_else(|| std::io::Error::other(String::from_utf8_lossy(&mounted.stderr).trim().to_owned()))
}

pub fn local_path_within(uri: &str, wait: std::time::Duration) -> std::io::Result<PathBuf> {
    let info = gio(&["info", uri], wait)?;
    local_path(&String::from_utf8_lossy(&info.stdout)).ok_or_else(|| std::io::Error::other(String::from_utf8_lossy(&info.stderr).trim().to_owned()))
}

fn fuse_unmounter() -> &'static str {
    if crate::shell::on_path(std::ffi::OsStr::new("fusermount3")).is_none() && crate::shell::on_path(std::ffi::OsStr::new("fusermount")).is_some() { "fusermount" } else { "fusermount3" }
}

pub fn disconnect_command(point: &Path, table: &[Mount], gvfs_root: &Path) -> Vec<std::ffi::OsString> {
    let words = |w: &[&str]| w.iter().map(std::ffi::OsString::from).chain([point.as_os_str().to_owned()]).collect();
    if point.starts_with(gvfs_root) {
        return words(&["gio", "mount", "-u"]);
    }
    match table.iter().rev().find(|m| m.path() == point) {
        Some(m) if m.fstype.starts_with("fuse.") || m.fstype == "sshfs" => words(&[fuse_unmounter(), "-u"]),
        _ => words(&["umount"]),
    }
}

pub fn disconnect(point: &Path) -> std::io::Result<()> {
    disconnect_from(point, &mounts::read()?, &gvfs_root())
}

fn disconnect_from(point: &Path, table: &[Mount], gvfs_root: &Path) -> std::io::Result<()> {
    let [program, args @ ..] = &disconnect_command(point, table, gvfs_root)[..] else { return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput)) };
    let mut cmd = crate::tools::command(&program.to_string_lossy())?;
    cmd.args(args);
    let out = run_within(cmd, CONNECT_WAIT)?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    Err(std::io::Error::other(if why.contains("busy") { S::ErrorsFsDriveBusy.en().to_owned() } else { why }))
}

fn unmounted() -> Vec<Drive> {
    let out = std::process::Command::new("lsblk").args(["-P", "-b", "-o", "PATH,LABEL,MOUNTPOINT,HOTPLUG,FSTYPE,TYPE,PARTTYPE,KNAME,PKNAME"]).stderr(std::process::Stdio::null()).output();
    out.map(|o| unmounted_from(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

const ENCRYPTED: [&str; 2] = ["crypto_LUKS", "BitLocker"];
const NOT_DATA: [&str; 9] = ["", "swap", "crypto_LUKS", "LVM2_member", "linux_raid_member", "zfs_member", "bcache", "squashfs", "BitLocker"];
const SYSTEM_PARTITIONS: [&str; 7] = [
    "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
    "0xef",
    "e3c9e316-0b5c-4db8-817d-f92df00215ae",
    "de94bba4-06d1-4d40-a16a-bfd50179d6ac",
    "21686148-6449-6e6f-744e-656564454649",
    "bc13c2ff-59e6-4262-a352-b275fd6f7172",
    "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f",
];

fn data_partition(f: &HashMap<&str, &str>) -> bool {
    let get = |k: &str| f.get(k).copied().unwrap_or("");
    get("MOUNTPOINT").is_empty() && !NOT_DATA.contains(&get("FSTYPE")) && get("TYPE") != "loop" && !SYSTEM_PARTITIONS.contains(&get("PARTTYPE").to_ascii_lowercase().as_str())
}

fn fields(line: &str) -> HashMap<&str, &str> {
    let mut f = HashMap::new();
    let mut rest = line.trim();
    while let Some((key, tail)) = rest.split_once("=\"") {
        let Some((value, after)) = tail.split_once('"') else { break };
        f.insert(key.trim(), value);
        rest = after;
    }
    f
}

pub fn unmounted_from(lsblk: &str) -> Vec<Drive> {
    let lines: Vec<HashMap<&str, &str>> = lsblk.lines().map(fields).collect();
    let unlocked = |kname: &str| !kname.is_empty() && lines.iter().any(|l| l.get("PKNAME") == Some(&kname) && l.get("TYPE") == Some(&"crypt"));
    let mut out = Vec::new();
    for f in &lines {
        let get = |k: &str| f.get(k).copied().unwrap_or("");
        let locked = get("MOUNTPOINT").is_empty() && ENCRYPTED.contains(&get("FSTYPE")) && !unlocked(get("KNAME"));
        if !data_partition(f) && !locked {
            continue;
        }
        let Some(path) = flit_core::FsPath::parse_exact(get("PATH").as_bytes()) else { continue };
        let short = get("PATH").rsplit('/').next().unwrap_or("");
        let (label, kind) = match (get("LABEL"), locked) {
            ("", true) => (format!("{} ({short})", S::SidebarDriveEncrypted.en()), Some(S::SidebarDriveEncrypted)),
            ("", false) => (short.to_owned(), None),
            (label, _) => (label.to_owned(), None),
        };
        out.push(Drive { path, label, total: 0, free: 0, file_system: get("FSTYPE").to_owned(), removable: get("HOTPLUG") == "1", unmounted: true, kind, remote: None });
    }
    out
}

pub fn drive_error(key: S, drive: &Path, e: &std::io::Error) -> PlatformError {
    if e.kind() == std::io::ErrorKind::Interrupted {
        return PlatformError::new(-1, flit_platform::CANCELLED);
    }
    PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, flit_core::tr_en!(key, drive = drive.display(), why = flit_core::fmt::io_reason(e)))
}

pub fn eject_error(drive: &Path, e: &std::io::Error) -> PlatformError {
    drive_error(S::ErrorsFsEjectFailed, drive, e)
}

pub fn list_computer(out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let table = mounts::read().map_err(|e| FsError { kind: FsErrorKind::Unavailable, code: e.raw_os_error().unwrap_or(-1) as i64 })?;
    let volumes = volumes_from(&table, &mounts::user_name(), Path::new(mounts::BY_LABEL));
    computer_from(out, &volumes, &unmounted().into_iter().chain(gvfs(&gvfs_root())).chain(phones(&gvfs_root())).collect::<Vec<_>>());
    Ok(())
}

pub fn computer_from(out: &mut ListingBuilder<'_>, volumes: &[Volume], others: &[Drive]) {
    let labelled = |out: &mut ListingBuilder<'_>, label: &str| {
        let i = out.listing.len() as u32 - 1;
        out.listing.set_label(i, label);
    };
    for v in volumes {
        let mut meta = Meta { attrs: attr::DIR | attr::VOLUME, ..Default::default() };
        if v.total > 0 {
            meta.size = v.total;
            meta.attrs |= attr::SIZE_KNOWN;
        }
        out.push(&v.mount.point, meta);
        if v.total > 0 {
            out.listing.push_drive_info(v.free, &v.mount.fstype);
        }
        labelled(out, &v.label);
    }
    for d in others {
        out.push(d.path.as_bytes(), Meta { attrs: attr::DIR | attr::VOLUME, ..Default::default() });
        labelled(out, &d.title());
    }
    out.keys.relabel(&out.listing);
    out.batch_done();
}

pub fn watch_mounts(changed: Box<dyn Fn() + Send>) {
    if let Err(e) = mounts::watch(Path::new(mounts::MOUNTINFO), changed) {
        eprintln!("flit: cannot watch {}: {e}", mounts::MOUNTINFO);
    }
}

pub fn storage_tops() -> Vec<PathBuf> {
    mounts::read().unwrap_or_default().iter().filter(|m| m.is_storage()).map(|m| m.path().to_path_buf()).collect()
}

pub fn trash_tops() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let table = mounts::read().unwrap_or_default();
    let tops = table.iter().filter(|m| m.is_storage()).map(|m| m.path().to_path_buf()).collect();
    let volumes = mounts::disks(&table, &mounts::user_name()).iter().filter(|m| m.is_storage()).map(|m| m.path().to_path_buf()).collect();
    (tops, volumes)
}

pub fn trash() -> Result<Trash, PlatformError> {
    Trash::from_env().ok_or_else(|| PlatformError::new(-1, flit_core::tr_en!(S::ErrorsOpsTrashUnknown)))
}

pub fn trash_folders(trash: &Trash, tops: &[PathBuf], volumes: &[PathBuf]) -> Vec<PathBuf> {
    let nearest = |files: &Path| files.ancestors().find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_dir())).map(Path::to_path_buf);
    let found = trash.dirs(tops).into_iter().filter_map(|dir| nearest(&dir.join("files")));
    let coming = volumes.iter().flat_map(|v| trash.volume_dirs(v).into_iter().filter_map(move |dir| nearest(&dir.join("files")).filter(|p| p.starts_with(v))));
    let mut out: Vec<PathBuf> = Vec::new();
    for existing in found.chain(coming) {
        if !out.contains(&existing) {
            out.push(existing);
        }
    }
    out
}

pub fn list_trash(out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let trash = trash().map_err(|_| FsError { kind: FsErrorKind::Unavailable, code: 0 })?;
    trash.list(&trash.dirs(&storage_tops()), out);
    Ok(())
}

fn failure(p: &Path, e: std::io::Error) -> PlatformError {
    PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("{}: {}", p.display(), flit_core::fmt::io_reason(&e)))
}

pub fn purge(trash: &Trash, pairs: &[(flit_core::FsPath, flit_core::FsPath)]) -> OpOutcome {
    let mut out = OpOutcome::default();
    for (trashed, original) in pairs {
        let Some(path) = flit_platform_linux::host(trashed) else {
            out.error = Some(PlatformError::new(-1, flit_core::tr_en!(S::ErrorsOpsTrashForeign, path = trashed.display())));
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
        return OpOutcome { items: Vec::new(), error: Some(PlatformError::new(-1, flit_core::tr_en!(S::ErrorsOpsTrashAlreadyEmpty))) };
    }
    let items = removed.iter().filter_map(|p| crate::xdg::fs_path(p)).map(|p| (p.clone(), p)).collect();
    OpOutcome { items, error: error.map(|e| PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, flit_core::tr_en!(S::ErrorsOpsTrashPartial, error = flit_core::fmt::io_reason(&e)))) }
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

const XBEL_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel version=\"1.0\"\n      xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\"\n      xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\"\n>\n";
const XBEL_TAIL: &str = "</xbel>";
const OWN_APP: &str = "<bookmark:application name=\"Flit\"";
const APPS_END: &str = "</bookmark:applications>";
const PRIVACY_FOR: std::time::Duration = std::time::Duration::from_secs(60);

pub fn recent_href(path: &[u8]) -> String {
    let keep = |b: u8| b.is_ascii_alphanumeric() || b"-._~!$()*+,;=:@/".contains(&b);
    let encoded: String = path.iter().map(|&b| if keep(b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    format!("file://{encoded}")
}

pub fn xbel_stamp(t: std::time::SystemTime) -> String {
    let since = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::gmtime_r(&secs, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, since.subsec_micros())
}

fn set_attr(tag: &str, name: &str, value: &str) -> String {
    let key = format!(" {name}=\"");
    let found = tag.find(&key).and_then(|at| Some((at + key.len(), at + key.len() + tag[at + key.len()..].find('"')?)));
    match found {
        Some((start, end)) => format!("{}{value}{}", &tag[..start], &tag[end..]),
        None => format!("{}{key}{value}\">", tag.trim_end_matches('>')),
    }
}

fn find_bookmark(xbel: &str, path: &[u8]) -> Option<(usize, usize, usize)> {
    xbel.match_indices("<bookmark ").find_map(|(start, _)| {
        let open_end = start + xbel[start..].find('>')? + 1;
        let href = unescape_xml(attribute(&xbel.as_bytes()[start..open_end], b"href")?);
        if crate::instance::decode_uri(&href)? != path {
            return None;
        }
        Some((start, open_end, open_end + xbel[open_end..].find("</bookmark>")?))
    })
}

pub fn remember_in_xbel(xbel: &str, path: &[u8], mime: &str, stamp: &str) -> Option<String> {
    let own = |count: u64| format!("{OWN_APP} exec=\"&apos;flit %u&apos;\" modified=\"{stamp}\" count=\"{count}\"/>");
    if let Some((start, open_end, close)) = find_bookmark(xbel, path) {
        let tag = set_attr(&set_attr(&xbel[start..open_end], "modified", stamp), "visited", stamp);
        let body = &xbel[open_end..close];
        let body = match (body.find(OWN_APP), body.find(APPS_END)) {
            (Some(at), _) => {
                let end = at + body[at..].find("/>")? + 2;
                let count = attribute(&body.as_bytes()[at..end], b"count").and_then(|c| std::str::from_utf8(c).ok()?.parse::<u64>().ok()).unwrap_or(0);
                format!("{}{}{}", &body[..at], own(count + 1), &body[end..])
            }
            (None, Some(at)) => format!("{}  {}\n        {}", body[..at].trim_end_matches([' ', '\t']), own(1), &body[at..]),
            (None, None) => body.to_owned(),
        };
        return Some(format!("{}{tag}{body}{}", &xbel[..start], &xbel[close..]));
    }
    let href = recent_href(path);
    let block = format!("  <bookmark href=\"{href}\" added=\"{stamp}\" modified=\"{stamp}\" visited=\"{stamp}\">\n    <info>\n      <metadata owner=\"http://freedesktop.org\">\n        <mime:mime-type type=\"{mime}\"/>\n        <bookmark:applications>\n          {}\n        </bookmark:applications>\n      </metadata>\n    </info>\n  </bookmark>\n", own(1));
    if xbel.trim().is_empty() {
        return Some(format!("{XBEL_HEAD}{block}{XBEL_TAIL}\n"));
    }
    let at = xbel.rfind(XBEL_TAIL).filter(|_| xbel.contains("<xbel"))?;
    Some(format!("{}{block}{}", &xbel[..at], &xbel[at..]))
}

pub fn privacy_allows_recent(remember: Option<&str>, max_age: Option<&str>) -> bool {
    let off = remember.is_some_and(|r| r.trim().ends_with("false"));
    let none_kept = max_age.and_then(|a| a.split_whitespace().last()?.parse::<i64>().ok()) == Some(0);
    !off && !none_kept
}

fn privacy_now() -> bool {
    static SEEN: std::sync::Mutex<Option<(std::time::Instant, bool)>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, allowed)) = *seen {
        if at.elapsed() < PRIVACY_FOR {
            return allowed;
        }
    }
    let ask = |key: &str| crate::shell::gsettings(&["org.gnome.desktop.privacy", key]);
    let allowed = privacy_allows_recent(ask("remember-recent-files").as_deref(), ask("recent-files-max-age").as_deref());
    *seen = Some((std::time::Instant::now(), allowed));
    allowed
}

pub fn remember_recent_in(file: &Path, path: &Path, mime: &str, now: std::time::SystemTime) -> std::io::Result<bool> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let old = match std::fs::read(file) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let Some(new) = remember_in_xbel(&old, path.as_os_str().as_bytes(), mime, &xbel_stamp(now)) else { return Ok(false) };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = file.with_file_name(format!(".recently-used.xbel.flit-{}", std::process::id()));
    let mut out = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temp)?;
    out.write_all(new.as_bytes())?;
    drop(out);
    std::fs::rename(&temp, file)?;
    Ok(true)
}

pub fn remember_recent(paths: Vec<PathBuf>) {
    if paths.is_empty() || !flit_platform::remember_recent() {
        return;
    }
    let _ = std::thread::Builder::new().name("flit-recent".into()).spawn(move || {
        let Some(file) = crate::xdg::data_home().map(|d| d.join("recently-used.xbel")) else { return };
        if !privacy_now() {
            return;
        }
        for p in paths.iter().filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file())) {
            if let Err(e) = remember_recent_in(&file, p, &crate::mime::mime_of_file(p), std::time::SystemTime::now()) {
                tracing::debug!("recently-used.xbel not updated: {e}");
                return;
            }
        }
    });
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
