#[path = "xbel.rs"]
mod xbel;

use xbel::{attribute, attribute_range, bookmark_href, bookmark_tags, unescape_xml};

use flit_core::i18n::S;
use flit_core::listing::{attr, Meta};
use flit_core::ui::Drive;
use flit_core::{FsError, FsErrorKind, ListingBuilder};
use flit_platform::{OpOutcome, PlatformError};
use flit_platform_linux::mounts::{self, Mount};
use flit_platform_linux::Trash;
use std::os::unix::ffi::OsStrExt;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::path::{Path, PathBuf};

pub struct Volume {
    pub mount: Mount,
    pub label: String,
    pub total: u64,
    pub free: u64,
    pub offline: bool,
}

fn name_of(m: &Mount, l: flit_core::i18n::Lang) -> String {
    match m.point.as_slice() {
        b"/" => flit_core::i18n::S::SidebarFolderFileSystem.in_lang(l).into(),
        p => String::from_utf8_lossy(p.rsplit(|&b| b == b'/').next().unwrap_or(p)).into_owned(),
    }
}

type Probe = fn(&Path) -> Option<(u64, u64)>;

type FailedShares = HashMap<PathBuf, u32>;
static FAILED_SHARES: LazyLock<Mutex<FailedShares>> = LazyLock::new(Mutex::default);

fn spaces_of(ms: &[Mount], failed: &mut FailedShares, wait: std::time::Duration, probe: Probe) -> Vec<Option<(u64, u64)>> {
    failed.retain(|path, id| ms.iter().any(|m| m.id == *id && m.path() == path));
    let remote: Vec<PathBuf> = ms.iter().filter(|m| m.is_network() && !failed.contains_key(m.path())).map(|m| m.path().to_path_buf()).collect();
    let mut probed = mounts::spaces_within(&remote, wait, probe).into_iter();
    ms.iter()
        .map(|m| {
            if !m.is_network() {
                return probe(m.path());
            }
            if failed.contains_key(m.path()) {
                return None;
            }
            let space = probed.next().flatten();
            if space.is_none() {
                failed.insert(m.path().to_owned(), m.id);
            }
            space
        })
        .collect()
}

pub fn volumes_from(table: &[Mount], user: &[u8], by_label: &Path) -> Vec<Volume> {
    let labels = mounts::labels(by_label);
    let disks = mounts::disks(table, user);
    let spaces = spaces_of(&disks, &mut FAILED_SHARES.lock().unwrap_or_else(|e| e.into_inner()), mounts::NETWORK_SPACE_WAIT, mounts::space);
    disks
        .into_iter()
        .zip(spaces)
        .map(|(m, space)| {
            let (total, free) = space.unwrap_or_default();
            let label = mounts::label_of(&m, &labels).filter(|_| m.point != b"/").unwrap_or_else(|| name_of(&m, flit_core::i18n::lang()));
            Volume { offline: m.is_network() && space.is_none(), mount: m, label, total, free }
        })
        .collect()
}

fn dormant_shares(table: &[Mount], fstab: &[u8]) -> Vec<Drive> {
    let mut drives = Vec::new();
    for line in fstab.split(|&b| b == b'\n') {
        let mut fields = line.split(u8::is_ascii_whitespace).filter(|f| !f.is_empty());
        let (Some(source), Some(target), Some(fs)) = (fields.next(), fields.next(), fields.next()) else { continue };
        if source.starts_with(b"#") {
            continue;
        }
        let point = mounts::unescape(target);
        let Some(m) = table.iter().rev().find(|m| m.point == point).filter(|m| m.fstype == "autofs") else { continue };
        let remote = Mount { fstype: String::from_utf8_lossy(fs).into_owned(), source: mounts::unescape(source), ..m.clone() };
        if remote.is_network() {
            let volume = Volume { label: name_of(&remote, flit_core::i18n::lang()), mount: remote, total: 0, free: 0, offline: true };
            drives.extend(drive_of(&volume));
        }
    }
    drives
}

fn other_drives(table: &[Mount]) -> Vec<Drive> {
    let dormant = dormant_shares(table, &std::fs::read("/etc/fstab").unwrap_or_default());
    dormant.into_iter().chain(unmounted()).chain(gvfs(&gvfs_root())).chain(phones(&gvfs_root())).collect()
}

fn retry_root<'a>(point: &Path, table: &'a [Mount]) -> Option<&'a Path> {
    mounts::enclosing(point, table).filter(|m| m.is_network() || m.fstype == "autofs").map(Mount::path)
}

pub fn inactive_share(path: &Path) -> bool {
    let table = mounts::read().unwrap_or_default();
    let Some(m) = mounts::enclosing(path, &table) else { return false };
    m.fstype == "autofs" || (m.is_network() && FAILED_SHARES.lock().unwrap_or_else(|e| e.into_inner()).get(m.path()) == Some(&m.id))
}

pub fn retry_share(point: &Path) -> Option<std::io::Result<bool>> {
    let table = mounts::read().ok()?;
    let root = retry_root(point, &table)?;
    Some(probe_share(point, root))
}

fn probe_share(point: &Path, root: &Path) -> std::io::Result<bool> {
    let mut cmd = crate::tools::command("stat")?;
    cmd.args(["-L", "-c", "%f", "--"]).arg(if point == root { root.join(".") } else { point.to_owned() });
    let out = run_within(cmd, CONNECT_WAIT)?;
    if !out.status.success() {
        return Err(std::io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    let mode = u32::from_str_radix(String::from_utf8_lossy(&out.stdout).trim(), 16).map_err(std::io::Error::other)?;
    FAILED_SHARES.lock().unwrap_or_else(|e| e.into_inner()).remove(root);
    Ok(mode & libc::S_IFMT == libc::S_IFDIR)
}

pub fn drive_of(v: &Volume) -> Option<Drive> {
    let remote = v.mount.is_network().then(|| String::from_utf8_lossy(&v.mount.source).into_owned());
    Some(Drive { path: crate::xdg::fs_path(v.mount.path())?, label: v.label.clone(), total: v.total, free: v.free, file_system: v.mount.fstype.clone(), removable: v.mount.removable(), unmounted: v.offline, kind: None, remote })
}

pub fn drives() -> Vec<Drive> {
    let table = mounts::read().unwrap_or_default();
    volumes_from(&table, &mounts::user_name(), Path::new(mounts::BY_LABEL)).iter().filter_map(drive_of).chain(other_drives(&table)).chain([network_drive()]).collect()
}

pub fn network_drive() -> Drive {
    Drive { path: flit_core::FsPath::network(), label: S::PathNetwork.en().to_owned(), total: 0, free: 0, file_system: String::new(), removable: false, unmounted: false, kind: Some(S::PathNetwork), remote: None }
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

pub fn run_within(cmd: std::process::Command, wait: std::time::Duration) -> std::io::Result<std::process::Output> {
    run_command(cmd, wait, None::<fn(&str)>)
}

pub fn run_streaming(cmd: std::process::Command, wait: std::time::Duration, each: impl FnMut(&str) + Send + 'static) -> std::io::Result<std::process::Output> {
    run_command(cmd, wait, Some(each))
}

fn run_command(mut cmd: std::process::Command, wait: std::time::Duration, each: Option<impl FnMut(&str) + Send + 'static>) -> std::io::Result<std::process::Output> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::{Output, Stdio};
    fn drain(mut pipe: impl Read + Send + 'static) -> std::io::Result<std::thread::JoinHandle<std::io::Result<Vec<u8>>>> {
        std::thread::Builder::new().spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let deadline = std::time::Instant::now() + wait;
    let result = (|| {
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return Err(std::io::Error::other(S::ErrorsFsNotResponding.en()));
        };
        let stdout = match each {
            None => drain(stdout)?,
            Some(mut each) => std::thread::Builder::new().spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = Vec::new();
                while reader.read_until(b'\n', &mut line)? != 0 {
                    each(String::from_utf8_lossy(&line).trim_end());
                    line.clear();
                }
                Ok(Vec::new())
            })?,
        };
        let stderr = drain(stderr)?;
        loop {
            if stdout.is_finished() && stderr.is_finished() {
                if let Some(status) = child.try_wait()? {
                    let output = |h: std::thread::JoinHandle<_>| h.join().map_err(|_| std::io::Error::other(S::ErrorsFsNotResponding.en()));
                    return Ok(Output { status, stdout: output(stdout)??, stderr: output(stderr)?? });
                }
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, S::ErrorsFsNotResponding.en()));
            }
            std::thread::sleep(left.min(std::time::Duration::from_millis(50)));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        crate::shell::reap(child)?;
    }
    result
}

pub fn needs_login(out: &std::process::Output) -> bool {
    let said = [&out.stdout[..], &out.stderr[..]].concat();
    let text = String::from_utf8_lossy(&said);
    !out.status.success() && ["Authentication Required", "Password:", "Password dialog cancelled"].iter().any(|w| text.contains(w))
}

pub(crate) fn gio_command(args: &[&str]) -> std::io::Result<std::process::Command> {
    let program = crate::tools::require("gio").map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.message))?;
    Ok(gio_at(&program, args))
}

pub(crate) fn gio_at(program: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).env("LC_ALL", "C").env("LANGUAGE", "");
    cmd
}

fn gio(args: &[&str], wait: std::time::Duration) -> std::io::Result<std::process::Output> {
    run_within(gio_command(args)?, wait)
}

pub fn connect(uri: &str) -> std::io::Result<PathBuf> {
    let mounted = gio(&["mount", uri], CONNECT_WAIT)?;
    if needs_login(&mounted) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, S::ErrorsFsNeedsLogin.en()));
    }
    let info = gio(&["info", uri], CONNECT_WAIT)?;
    local_path(&String::from_utf8_lossy(&info.stdout)).ok_or_else(|| std::io::Error::other(String::from_utf8_lossy(&mounted.stderr).trim().to_owned()))
}

pub fn mount_only(uri: &str) -> std::io::Result<()> {
    let mounted = gio(&["mount", uri], CONNECT_WAIT)?;
    if needs_login(&mounted) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, S::ErrorsFsNeedsLogin.en()));
    }
    let said = String::from_utf8_lossy(&mounted.stderr);
    if mounted.status.success() || said.contains("already mounted") {
        return Ok(());
    }
    Err(std::io::Error::other(said.trim().to_owned()))
}

pub fn local_path_within(uri: &str, wait: std::time::Duration) -> std::io::Result<PathBuf> {
    let info = gio(&["info", uri], wait)?;
    local_path(&String::from_utf8_lossy(&info.stdout)).ok_or_else(|| std::io::Error::other(String::from_utf8_lossy(&info.stderr).trim().to_owned()))
}

fn fuse_unmounter() -> &'static str {
    if crate::shell::on_path(std::ffi::OsStr::new("fusermount3")).is_none() && crate::shell::on_path(std::ffi::OsStr::new("fusermount")).is_some() {
        "fusermount"
    } else {
        "fusermount3"
    }
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
const SYSTEM_PARTITIONS: [&str; 7] =
    ["c12a7328-f81f-11d2-ba4b-00a0c93ec93b", "0xef", "e3c9e316-0b5c-4db8-817d-f92df00215ae", "de94bba4-06d1-4d40-a16a-bfd50179d6ac", "21686148-6449-6e6f-744e-656564454649", "bc13c2ff-59e6-4262-a352-b275fd6f7172", "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f"];

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
        return PlatformError::cancelled(-1, flit_platform::CANCELLED);
    }
    PlatformError::from_io(e, flit_core::tr_en!(key, drive = drive.display(), why = flit_core::fmt::io_reason(e)))
}

pub fn eject_error(drive: &Path, e: &std::io::Error) -> PlatformError {
    drive_error(S::ErrorsFsEjectFailed, drive, e)
}

pub fn list_computer(out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let table = mounts::read().map_err(|e| FsError { kind: FsErrorKind::Unavailable, code: e.raw_os_error().unwrap_or(-1) as i64 })?;
    let volumes = volumes_from(&table, &mounts::user_name(), Path::new(mounts::BY_LABEL));
    computer_from(out, &volumes, &other_drives(&table));
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
        if v.offline {
            if let Some(d) = drive_of(v) {
                labelled(out, &d.title());
            }
        } else {
            labelled(out, &v.label);
        }
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
    PlatformError::from_io(&e, format!("{}: {}", p.display(), flit_core::fmt::io_reason(&e)))
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
    OpOutcome { items, error: error.map(|e| PlatformError::from_io(&e, flit_core::tr_en!(S::ErrorsOpsTrashPartial, error = flit_core::fmt::io_reason(&e)))) }
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
    let gtk = gtk.split(|&b| b == b'\n').filter_map(|l| l.strip_suffix(b"\r").unwrap_or(l).split(|&b| b == b' ').next());
    let kde = bookmark_tags(kde).filter_map(bookmark_href);
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
    let mounts = mounts::read().unwrap_or_default();
    bookmarks_from(&gtk, &kde, &[home.as_bytes(), b"/"], |p| mounts::remote_path(p, &mounts) || p.is_dir())
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
    match attribute_range(tag.as_bytes(), name.as_bytes()) {
        Some(range) => format!("{}{value}{}", &tag[..range.start], &tag[range.end..]),
        None => format!("{} {name}=\"{value}\">", tag.trim_end_matches('>')),
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

pub fn recent_from(xbel: &[u8], limit: usize, exists: impl Fn(&Path, Option<bool>) -> Option<bool>) -> Vec<flit_core::ui::Recent> {
    let xbel = String::from_utf8_lossy(xbel);
    let mut found: Vec<_> = xbel
        .match_indices("<bookmark ")
        .filter_map(|(start, _)| {
            let (tag, tail) = xbel[start..].split_once('>')?;
            let body = if tag.ends_with('/') { "" } else { tail.split_once("</bookmark>").map_or("", |(body, _)| body) };
            let tag = tag.as_bytes();
            let href = bookmark_href(tag)?;
            let path = crate::instance::decode_uri(&href).filter(|p| p.starts_with(b"/"))?;
            let at = attribute(tag, b"modified").or_else(|| attribute(tag, b"visited")).and_then(utc_ticks).unwrap_or(0);
            let dir = body.split_once("<mime:mime-type").and_then(|(_, tail)| tail.split_once('>')).and_then(|(attrs, _)| attribute(attrs.as_bytes(), b"type")).map(|mime| mime == b"inode/directory");
            Some((at, path, dir))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut out: Vec<flit_core::ui::Recent> = Vec::new();
    for (at, path, saved_dir) in found {
        if out.len() == limit {
            break;
        }
        let Some(target) = flit_core::FsPath::parse_exact(&path) else { continue };
        if out.iter().any(|r| r.target == target) {
            continue;
        }
        let Some(dir) = exists(Path::new(std::ffi::OsStr::from_bytes(&path)), saved_dir) else { continue };
        let name = flit_core::name::display(target.file_name()).into_owned();
        out.push(flit_core::ui::Recent { name, target, dir, at });
    }
    out
}

pub fn recent(limit: usize) -> (Vec<flit_core::ui::Recent>, bool) {
    let file = crate::xdg::data_home().map(|d| d.join("recently-used.xbel"));
    let bytes = file.and_then(|f| std::fs::read(f).ok()).unwrap_or_default();
    let mounts = mounts::read().unwrap_or_default();
    (recent_from(&bytes, limit, |p, saved_dir| if mounts::remote_path(p, &mounts) { saved_dir } else { std::fs::metadata(p).ok().map(|m| m.is_dir()) }), false)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn folder_key(p: &[u8]) -> &[u8] {
    let mut p = p;
    while p.len() > 1 && p.ends_with(b"/") {
        p = &p[..p.len() - 1];
    }
    p
}

fn names(list: &[Vec<u8>], path: &[u8]) -> bool {
    list.iter().any(|x| folder_key(x) == folder_key(path))
}

fn gtk_line_path(line: &str) -> Option<Vec<u8>> {
    crate::instance::decode_uri(line.split(' ').next()?.as_bytes()).filter(|p| p.starts_with(b"/"))
}

fn line_ending(text: &str) -> &'static str {
    if text.split_once('\n').is_some_and(|(line, _)| line.ends_with('\r')) {
        "\r\n"
    } else {
        "\n"
    }
}

pub fn gtk_bookmarks_edit(text: &str, add: &[Vec<u8>], remove: &[Vec<u8>]) -> Option<String> {
    let all = text.lines().count();
    let kept: Vec<&str> = text.lines().filter(|l| !gtk_line_path(l).is_some_and(|p| names(remove, &p))).collect();
    let mut fresh: Vec<Vec<u8>> = Vec::new();
    for p in add {
        if !kept.iter().any(|l| gtk_line_path(l).is_some_and(|q| folder_key(&q) == folder_key(p))) && !names(&fresh, p) {
            fresh.push(p.clone());
        }
    }
    if kept.len() == all && fresh.is_empty() {
        return None;
    }
    let newline = line_ending(text);
    let mut out = String::with_capacity(text.len());
    for line in kept {
        out.push_str(line);
        out.push_str(newline);
    }
    for p in &fresh {
        out.push_str(&recent_href(p));
        out.push_str(newline);
    }
    Some(out)
}

struct KdePlace {
    start: usize,
    end: usize,
    path: Option<Vec<u8>>,
    system: bool,
}

fn kde_places(xbel: &str) -> Option<Vec<KdePlace>> {
    if !xbel.trim_end().ends_with(XBEL_TAIL) || !xbel.contains("<xbel") {
        return None;
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = xbel[from..].find("<bookmark ") {
        let start = from + at;
        let open_end = start + xbel[start..].find('>')? + 1;
        let tag = &xbel[start..open_end];
        if tag[1..].contains('<') {
            return None;
        }
        let (end, body) = if tag.ends_with("/>") {
            (open_end, "")
        } else {
            let close = open_end + xbel[open_end..].find("</bookmark>")?;
            let body = &xbel[open_end..close];
            if body.contains("<bookmark ") {
                return None;
            }
            (close + "</bookmark>".len(), body)
        };
        let path = attribute(tag.as_bytes(), b"href").and_then(|h| crate::instance::decode_uri(&unescape_xml(h))).filter(|p| p.starts_with(b"/"));
        out.push(KdePlace { start, end, path, system: body.contains("<isSystemItem>true</isSystemItem>") });
        from = end;
    }
    let closed = out.iter().filter(|p| xbel[..p.end].ends_with("</bookmark>")).count();
    (xbel.matches("</bookmark>").count() == closed).then_some(out)
}

fn kde_block(path: &[u8], id: &str) -> String {
    let href = recent_href(path);
    let name = path.rsplit(|&b| b == b'/').find(|c| !c.is_empty()).map(String::from_utf8_lossy).unwrap_or_default();
    let title = escape_xml(&name);
    format!(" <bookmark href=\"{href}\">\n  <title>{title}</title>\n  <info>\n   <metadata owner=\"http://freedesktop.org\">\n    <bookmark:icon name=\"folder\"/>\n   </metadata>\n   <metadata owner=\"http://www.kde.org\">\n    <ID>{id}</ID>\n   </metadata>\n  </info>\n </bookmark>\n")
}

fn line_start(text: &str, at: usize) -> usize {
    let line = text[..at].rfind('\n').map_or(0, |n| n + 1);
    if text[line..at].trim().is_empty() {
        line
    } else {
        at
    }
}

pub fn kde_places_edit(xbel: &str, add: &[Vec<u8>], remove: &[Vec<u8>], now_secs: u64) -> Option<String> {
    let places = kde_places(xbel)?;
    let mut out = xbel.to_owned();
    let mut changed = false;
    for place in places.iter().rev() {
        if place.system || !place.path.as_ref().is_some_and(|p| names(remove, p)) {
            continue;
        }
        let tail = out[place.end..].trim_start_matches([' ', '\t']);
        let end = match tail.as_bytes() {
            [b'\r', b'\n', ..] => out.len() - tail.len() + 2,
            [b'\n', ..] => out.len() - tail.len() + 1,
            _ => place.end,
        };
        out.replace_range(line_start(&out, place.start)..end, "");
        changed = true;
    }
    let mut fresh: Vec<Vec<u8>> = Vec::new();
    for p in add {
        let known = places.iter().filter_map(|k| k.path.as_deref()).any(|k| folder_key(k) == folder_key(p));
        if !known && !names(&fresh, p) {
            fresh.push(p.clone());
        }
    }
    if !fresh.is_empty() {
        let mut n = 0u64;
        let mut blocks = String::new();
        for p in &fresh {
            while out.contains(&format!("<ID>{now_secs}/{n}</ID>")) {
                n += 1;
            }
            blocks.push_str(&kde_block(p, &format!("{now_secs}/{n}")));
            n += 1;
        }
        let newline = line_ending(xbel);
        if newline == "\r\n" {
            blocks = blocks.replace('\n', newline);
        }
        let at = line_start(&out, out.rfind(XBEL_TAIL)?);
        if at > 0 && !out[..at].ends_with('\n') {
            blocks.insert_str(0, newline);
        }
        out.insert_str(at, &blocks);
        changed = true;
    }
    changed.then_some(out)
}

fn write_over(file: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let target = std::fs::canonicalize(file)?;
    let perms = std::fs::metadata(&target)?.permissions();
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = target.with_file_name(format!(".{name}.flit-{}", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
    let written = out.write_all(text.as_bytes()).and_then(|_| out.sync_all()).and_then(|_| std::fs::set_permissions(&temp, perms));
    drop(out);
    if let Err(e) = written.and_then(|_| std::fs::rename(&temp, &target)) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    Ok(())
}

fn edit_existing(file: &Path, edit: impl FnOnce(&str) -> Option<String>) -> std::io::Result<bool> {
    let old = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let Ok(old) = String::from_utf8(old) else { return Ok(false) };
    let Some(new) = edit(&old) else { return Ok(false) };
    write_over(file, &new)?;
    Ok(true)
}

pub fn sync_bookmarks_in(gtk: &Path, kde: &Path, add: &[Vec<u8>], remove: &[Vec<u8>], now_secs: u64) -> std::io::Result<(bool, bool)> {
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let gtk = edit_existing(gtk, |t| gtk_bookmarks_edit(t, add, remove));
    let kde = edit_existing(kde, |t| kde_places_edit(t, add, remove, now_secs));
    Ok((gtk?, kde?))
}

pub fn bookmark_paths(locs: &[flit_core::Location], skip: &[&[u8]]) -> Vec<Vec<u8>> {
    locs.iter()
        .filter_map(|l| match l {
            flit_core::Location::Local(p) => Some(p.as_bytes()),
            _ => None,
        })
        .filter(|p| p.starts_with(b"/") && !skip.iter().any(|s| folder_key(s) == folder_key(p)))
        .map(<[u8]>::to_vec)
        .collect()
}

type BookmarkJob = (Vec<Vec<u8>>, Vec<Vec<u8>>);

fn bookmark_worker(jobs: crossbeam_channel::Receiver<BookmarkJob>) {
    let is_dir = |p: &Vec<u8>| Path::new(std::ffi::OsStr::from_bytes(p)).is_dir();
    for (add, remove) in jobs {
        let add: Vec<Vec<u8>> = add.into_iter().filter(is_dir).collect();
        let remove: Vec<Vec<u8>> = remove.into_iter().filter(is_dir).collect();
        if add.is_empty() && remove.is_empty() {
            continue;
        }
        let (Some(gtk), Some(kde)) = (crate::xdg::config_home().map(|c| c.join("gtk-3.0/bookmarks")), crate::xdg::data_home().map(|d| d.join("user-places.xbel"))) else { continue };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if let Err(e) = sync_bookmarks_in(&gtk, &kde, &add, &remove, now) {
            tracing::debug!("GTK/KDE bookmarks not updated: {e}");
        }
    }
}

pub fn sync_bookmarks(added: &[flit_core::Location], removed: &[flit_core::Location]) {
    static JOBS: std::sync::OnceLock<Option<crossbeam_channel::Sender<BookmarkJob>>> = std::sync::OnceLock::new();
    let home = std::env::var_os("HOME").unwrap_or_default();
    let skip: [&[u8]; 2] = [home.as_bytes(), b"/"];
    let (add, remove) = (bookmark_paths(added, &skip), bookmark_paths(removed, &skip));
    if add.is_empty() && remove.is_empty() {
        return;
    }
    let jobs = JOBS.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new().name("flit-bookmarks".into()).spawn(move || bookmark_worker(rx)).ok().map(|_| tx)
    });
    if let Some(jobs) = jobs {
        let _ = jobs.send((add, remove));
    }
}

#[cfg(test)]
#[path = "../test/places.rs"]
mod tests;
