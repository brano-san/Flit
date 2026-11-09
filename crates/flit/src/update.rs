use crossbeam_channel::{unbounded, Receiver, Sender};
use ed25519_compact::{PublicKey, Signature};
use flit_core::i18n::S;
use flit_core::update::{base64_decode, newer, parse_manifest, percent, Release, UpdateEvent, UpdateRequest, CURRENT, MANIFEST_URL, PLATFORM};
use flit_platform::{Platform, PlatformError, Wake};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub const PUBLIC_KEY: &str = "0A2ecN/wJe1HI8lTnaDZBX6vFKWifiBOPaWoYWV6aEw=";
pub const BINARIES: &[&str] = if cfg!(windows) { &["flit.exe", "flit-indexd.exe"] } else { &["flit"] };
const RESTARTED_FROM: &str = "FLIT_RESTARTED_FROM";
const LOCAL_MANIFEST: &str = "FLIT_UPDATE_MANIFEST";
const QUIET_RUNS: [&str; 3] = ["FLIT_NO_UPDATE", "FLIT_HIDDEN", "FLIT_HIDDEN_CAPTURE"];
const OLD_SLOTS: usize = 8;
const PROBE: &str = ".flit-update-probe";
const EXIT_WAIT: Duration = Duration::from_secs(30);

static RESTARTED: AtomicBool = AtomicBool::new(false);
static RELAUNCH: OnceLock<PathBuf> = OnceLock::new();

pub enum Reply {
    Ui(UpdateEvent),
    Restart,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    ReadOnly(String),
    Failed(String),
}

pub fn network_allowed(metrics: bool) -> bool {
    !metrics && QUIET_RUNS.iter().all(|v| std::env::var_os(v).is_none())
}

pub fn decide(manifest: &[u8], platform: &str, current: &str) -> Result<Option<Release>, String> {
    let release = parse_manifest(&String::from_utf8_lossy(manifest), platform)?;
    Ok(newer(&release.version, current).then_some(release))
}

pub fn verify(bytes: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
    let bad = || S::UpdateErrorSignature.t().to_owned();
    let key = base64_decode(public_key).and_then(|k| PublicKey::from_slice(&k).ok()).ok_or_else(bad)?;
    let signature = base64_decode(signature).and_then(|s| Signature::from_slice(&s).ok()).ok_or_else(bad)?;
    key.verify(bytes, &signature).map_err(|_| bad())
}

fn missing(name: &str) -> String {
    flit_core::tr!(S::UpdateErrorMissing, name = name)
}

#[cfg(windows)]
pub fn binaries(archive: &[u8]) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(io::Cursor::new(archive)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for name in BINARIES {
        let index = (0..zip.len()).find(|&i| zip.by_index(i).is_ok_and(|f| f.is_file() && f.name().rsplit(['/', '\\']).next() == Some(*name))).ok_or_else(|| missing(name))?;
        let mut file = zip.by_index(index).map_err(|e| e.to_string())?;
        let mut bytes = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        out.push((*name, bytes));
    }
    Ok(out)
}

#[cfg(not(windows))]
pub fn binaries(archive: &[u8]) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
    use std::io::Read;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut found: Vec<(&'static str, Vec<u8>)> = Vec::new();
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let in_bin = path.parent().and_then(Path::file_name).is_some_and(|d| d == "bin");
        let Some(name) = BINARIES.iter().find(|n| in_bin && path.file_name().is_some_and(|f| f == **n)) else { continue };
        if entry.header().entry_type().is_file() && !found.iter().any(|(n, _)| n == name) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            found.push((name, bytes));
        }
    }
    BINARIES.iter().map(|name| found.iter().position(|(n, _)| n == name).map(|k| found.swap_remove(k)).ok_or_else(|| missing(name))).collect()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(suffix);
    path.with_file_name(name)
}

fn old_name(live: &Path, k: usize) -> PathBuf {
    with_suffix(live, &if k == 0 { ".old".to_owned() } else { format!(".old{k}") })
}

fn old_slot(live: &Path) -> PathBuf {
    for k in 0..OLD_SLOTS {
        let slot = old_name(live, k);
        let _ = fs::remove_file(&slot);
        if !slot.exists() {
            return slot;
        }
    }
    old_name(live, OLD_SLOTS)
}

pub fn remove_old(exe: &Path) {
    for live in BINARIES[1..].iter().map(|name| exe.with_file_name(name)).chain([exe.to_path_buf()]) {
        for k in 0..=OLD_SLOTS {
            let _ = fs::remove_file(old_name(&live, k));
        }
    }
}

pub fn writable(dir: &Path) -> bool {
    let probe = dir.join(PROBE);
    let made = fs::OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    made
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn replace(live: &Path, new: &Path) -> io::Result<Option<PathBuf>> {
    if !live.exists() {
        return fs::rename(new, live).map(|()| None);
    }
    let old = old_slot(live);
    fs::rename(live, &old)?;
    if let Err(e) = fs::rename(new, live) {
        let _ = fs::rename(&old, live);
        return Err(e);
    }
    Ok(Some(old))
}

pub fn swap(dir: &Path, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let staged: Vec<(PathBuf, PathBuf)> = files.iter().map(|(name, _)| (dir.join(name), with_suffix(&dir.join(name), ".new"))).collect();
    let discard = || {
        for (_, new) in &staged {
            let _ = fs::remove_file(new);
        }
    };
    for ((_, new), (_, bytes)) in staged.iter().zip(files) {
        if let Err(e) = write_new(new, bytes) {
            discard();
            return Err(format!("{}: {e}", new.display()));
        }
    }
    let mut swapped: Vec<(&Path, Option<PathBuf>)> = Vec::new();
    for (live, new) in &staged {
        match replace(live, new) {
            Ok(old) => swapped.push((live.as_path(), old)),
            Err(e) => {
                for (live, old) in swapped.iter().rev() {
                    let _ = fs::remove_file(live);
                    if let Some(old) = old {
                        let _ = fs::rename(old, live);
                    }
                }
                discard();
                return Err(format!("{}: {e}", live.display()));
            }
        }
    }
    Ok(())
}

pub fn install(release: &Release, exe: &Path, public_key: &str, download: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>, installing: &mut dyn FnMut()) -> Result<(), Failure> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    if !writable(dir) {
        return Err(Failure::ReadOnly(dir.display().to_string()));
    }
    let archive = download(&release.url).map_err(Failure::Failed)?;
    verify(&archive, &release.signature, public_key).map_err(Failure::Failed)?;
    installing();
    let mut files: Vec<(String, Vec<u8>)> = binaries(&archive).map_err(Failure::Failed)?.into_iter().map(|(name, bytes)| (name.to_owned(), bytes)).collect();
    if let (Some(first), Some(name)) = (files.first_mut(), exe.file_name()) {
        first.0 = name.to_string_lossy().into_owned();
    }
    swap(dir, &files).map_err(Failure::Failed)
}

fn fetch<P: Platform>(url: &str, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    P::download(url, &mut out, progress).map_err(|e: PlatformError| e.message)?;
    Ok(out)
}

fn check<P: Platform>(metrics: bool) -> Result<Option<Release>, String> {
    let manifest = match std::env::var_os(LOCAL_MANIFEST) {
        Some(path) => fs::read(path).map_err(|e| e.to_string())?,
        None if !network_allowed(metrics) => return Err(S::UpdateErrorOff.t().to_owned()),
        None => match fetch::<P>(MANIFEST_URL, &mut |_, _| true) {
            Err(e) if e.contains("404") => return Ok(None),
            fetched => fetched?,
        },
    };
    decide(&manifest, PLATFORM, CURRENT)
}

fn running_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(fs::canonicalize(&exe).unwrap_or(exe))
}


fn run<P: Platform>(jobs: Receiver<UpdateRequest>, tx: Sender<Reply>, wake: Wake, cancel: Arc<AtomicBool>, metrics: bool) {
    let send = |r: Reply| {
        let _ = tx.send(r);
        wake();
    };
    for job in jobs {
        let UpdateRequest::Install(release) = job else {
            if let UpdateRequest::Check { manual } = job {
                send(Reply::Ui(UpdateEvent::Checked { manual, found: check::<P>(metrics) }));
            }
            continue;
        };
        cancel.store(false, Ordering::Release);
        let mut shown = None;
        let mut download = |url: &str| {
            if !network_allowed(metrics) {
                return Err(S::UpdateErrorOff.t().to_owned());
            }
            fetch::<P>(url, &mut |received, total| {
                let step = percent(received, total).map_or(received >> 20, u64::from);
                if shown != Some(step) {
                    shown = Some(step);
                    send(Reply::Ui(UpdateEvent::Progress { received, total }));
                }
                !cancel.load(Ordering::Acquire)
            })
            .map_err(|e| if cancel.load(Ordering::Acquire) { S::UpdateErrorCancelled.t().to_owned() } else { e })
        };
        let result = match running_exe() {
            Ok(exe) => install(&release, &exe, PUBLIC_KEY, &mut download, &mut || send(Reply::Ui(UpdateEvent::Installing))).map(|()| exe),
            Err(e) => Err(Failure::Failed(e)),
        };
        match result {
            Ok(exe) => {
                tracing::info!("installed Flit {} into {}, restarting", release.version, exe.display());
                let _ = RELAUNCH.set(exe);
                send(Reply::Restart);
            }
            Err(Failure::ReadOnly(dir)) => send(Reply::Ui(UpdateEvent::ReadOnly(dir))),
            Err(Failure::Failed(e)) => {
                tracing::warn!("cannot install Flit {}: {e}", release.version);
                send(Reply::Ui(UpdateEvent::Failed(e)));
            }
        }
    }
}

pub struct UpdateWorker {
    jobs: Sender<UpdateRequest>,
    pub events: Receiver<Reply>,
    cancel: Arc<AtomicBool>,
}

impl UpdateWorker {
    pub fn start<P: Platform>(wake: Wake, metrics: bool) -> Result<Self, PlatformError> {
        let (jobs, rx) = unbounded();
        let (tx, events) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("flit-update".into())
            .spawn(move || run::<P>(rx, tx, wake, flag, metrics))
            .map_err(|e| PlatformError::new(-1, format!("cannot start flit-update: {e}")))?;
        Ok(Self { jobs, events, cancel })
    }

    pub fn submit(&self, req: UpdateRequest) {
        if req == UpdateRequest::Cancel {
            self.cancel.store(true, Ordering::Release);
        }
        let _ = self.jobs.send(req);
    }
}

pub fn on_start<P: Platform>() {
    if let Ok(pid) = std::env::var(RESTARTED_FROM) {
        std::env::remove_var(RESTARTED_FROM);
        if let Ok(pid) = pid.parse() {
            P::wait_exit(pid, EXIT_WAIT);
        }
        RESTARTED.store(true, Ordering::Release);
    }
    if let Ok(exe) = running_exe() {
        remove_old(&exe);
    }
}

pub fn restarted() -> bool {
    RESTARTED.load(Ordering::Acquire)
}

pub fn relaunch_if_installed() {
    let Some(exe) = RELAUNCH.get() else { return };
    if let Err(e) = std::process::Command::new(exe).env(RESTARTED_FROM, std::process::id().to_string()).spawn() {
        tracing::error!("cannot start the updated Flit {}: {e}", exe.display());
    }
}

#[cfg(test)]
#[path = "../test/update.rs"]
mod tests;
