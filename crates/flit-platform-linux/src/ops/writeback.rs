use crate::mounts::Mount;
use flit_core::FsPath;
use flit_platform::walk::Media;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::time::{Duration, Instant};

const MIB: usize = 1 << 20;
const START: usize = 4 * MIB;
const MAX: usize = 256 * MIB;
const TARGET: Duration = Duration::from_millis(250);
const SYS_BLOCK: &str = "/sys/dev/block";

struct Device {
    fs: Option<String>,
    through: Option<bool>,
    sync: Option<Volume>,
}

struct Volume {
    fd: OwnedFd,
    path: FsPath,
    window: Option<usize>,
    deferred: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CopyMode {
    Buffered,
    Deferred,
    Windowed(usize),
}

impl Volume {
    fn copy_mode(&self, bytes: u64) -> CopyMode {
        match self.window {
            Some(window) if bytes >= window as u64 => CopyMode::Windowed(window),
            _ => CopyMode::Deferred,
        }
    }
}

#[derive(Default)]
pub(super) struct Volumes {
    mounts: Option<Vec<Mount>>,
    devices: HashMap<u64, Device>,
}

fn device_numbers(dev: u64) -> (u32, u32) {
    ((dev >> 32) as u32, dev as u32)
}

fn slow(mounts: &[Mount], dev: (u32, u32), probe: impl Fn((u32, u32), Option<&Mount>) -> bool) -> bool {
    let matching = || mounts.iter().rev().filter(|m| m.dev == dev);
    if matching().any(|m| m.is_storage() && m.removable()) {
        return true;
    }
    let mount = matching().next();
    mount.is_none_or(Mount::is_storage) && probe(dev, mount)
}

fn block_slow(sys: &Path, dev: (u32, u32)) -> bool {
    let block = sys.join(format!("{}:{}", dev.0, dev.1));
    let removable = ["removable", "../removable"].iter().any(|p| std::fs::read_to_string(block.join(p)).is_ok_and(|t| t.trim() == "1"));
    removable || crate::walk::media(sys, libc::makedev(dev.0, dev.1)) == Media::Spinning
}

fn probe(dev: (u32, u32), mount: Option<&Mount>) -> bool {
    let sys = Path::new(SYS_BLOCK);
    if block_slow(sys, dev) {
        return true;
    }
    mount
        .filter(|m| m.source.starts_with(b"/dev/"))
        .and_then(|m| std::fs::metadata(Path::new(OsStr::from_bytes(&m.source))).ok())
        .filter(|m| m.file_type().is_block_device())
        .is_some_and(|m| block_slow(sys, (libc::major(m.rdev()), libc::minor(m.rdev()))))
}

impl Volumes {
    fn device(&mut self, dev: u64) -> &mut Device {
        self.devices.entry(dev).or_insert_with(|| {
            let mounts = self.mounts.get_or_insert_with(|| crate::mounts::read().unwrap_or_default());
            let fs = super::file_system(mounts, dev);
            Device { fs, through: None, sync: None }
        })
    }

    pub fn file_system(&mut self, dev: u64) -> Option<String> {
        self.device(dev).fs.clone()
    }

    pub fn open(&mut self, dev: u64, dir: BorrowedFd<'_>, dest: &FsPath, bytes: u64) -> io::Result<CopyMode> {
        self.device(dev);
        let device = self.devices.get_mut(&dev).unwrap();
        let through = *device.through.get_or_insert_with(|| slow(self.mounts.as_deref().unwrap_or_default(), device_numbers(dev), probe));
        if !through {
            return Ok(CopyMode::Buffered);
        }
        if device.sync.is_none() {
            device.sync = Some(Volume { fd: dir.try_clone_to_owned()?, path: dest.clone(), window: Some(START), deferred: 0 });
        }
        Ok(device.sync.as_ref().map_or(CopyMode::Buffered, |volume| volume.copy_mode(bytes)))
    }

    pub fn adjust(&mut self, dev: u64, window: Option<usize>) {
        if let Some(volume) = self.device(dev).sync.as_mut() {
            volume.window = window;
        }
    }

    pub fn defer(&mut self, dev: u64, bytes: u64) {
        if let Some(volume) = self.device(dev).sync.as_mut() {
            volume.deferred += bytes;
        }
    }

    pub fn finish(&mut self) -> (u64, Vec<(FsPath, io::Error)>) {
        self.finish_with(|fd| retry(|| unsafe { libc::syncfs(fd) }))
    }

    fn finish_with(&mut self, mut sync: impl FnMut(RawFd) -> io::Result<()>) -> (u64, Vec<(FsPath, io::Error)>) {
        let mut confirmed = 0;
        let mut failures = Vec::new();
        for volume in self.devices.values_mut().filter_map(|dev| dev.sync.take()) {
            match sync(volume.fd.as_raw_fd()) {
                Ok(()) => confirmed += volume.deferred,
                Err(error) => failures.push((volume.path, error)),
            }
        }
        (confirmed, failures)
    }
}

fn retry(mut call: impl FnMut() -> libc::c_int) -> io::Result<()> {
    loop {
        if call() == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Flush {
    Write,
    Wait,
}

pub(super) fn range(fd: RawFd, flush: Flush, offset: u64, bytes: usize) -> io::Result<()> {
    let flags = match flush {
        Flush::Write => libc::SYNC_FILE_RANGE_WRITE,
        Flush::Wait => libc::SYNC_FILE_RANGE_WAIT_BEFORE | libc::SYNC_FILE_RANGE_WAIT_AFTER,
    };
    retry(|| unsafe { libc::sync_file_range(fd, offset as _, bytes as _, flags) })
}

#[derive(Clone, Copy)]
struct Window {
    offset: u64,
    bytes: usize,
}

pub(super) struct Ranges {
    size: usize,
    writing: Option<Window>,
    pending: Option<(Window, Instant, bool)>,
    supported: bool,
}

impl Ranges {
    pub fn new(size: usize) -> Self {
        Self { size, writing: None, pending: None, supported: true }
    }

    pub fn size(&self) -> Option<usize> {
        self.supported.then_some(self.size)
    }

    pub fn room(&self) -> usize {
        self.size().map_or(usize::MAX, |size| size - self.writing.map_or(0, |window| window.bytes))
    }

    pub fn add(&mut self, offset: u64, bytes: usize) {
        if self.supported {
            self.writing.get_or_insert(Window { offset, bytes: 0 }).bytes += bytes;
        }
    }

    pub fn submit(&mut self, mut sync: impl FnMut(Flush, u64, usize) -> io::Result<()>, clock: impl Fn() -> Instant) -> io::Result<Option<u64>> {
        let Some(window) = self.writing.take() else { return Ok(None) };
        let full = window.bytes == self.size;
        let began = clock();
        match sync(Flush::Write, window.offset, window.bytes) {
            Ok(()) => {
                let written = self.wait(&mut sync, &clock)?;
                self.pending = Some((window, began, full));
                Ok(written)
            }
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOSYS | libc::EOPNOTSUPP | libc::EINVAL)) => {
                self.wait(&mut sync, &clock)?;
                self.supported = false;
                Ok(Some(window.offset + window.bytes as u64))
            }
            Err(e) => Err(e),
        }
    }

    pub fn wait(&mut self, mut sync: impl FnMut(Flush, u64, usize) -> io::Result<()>, clock: impl Fn() -> Instant) -> io::Result<Option<u64>> {
        let Some((window, began, full)) = self.pending.take() else { return Ok(None) };
        sync(Flush::Wait, window.offset, window.bytes)?;
        if full {
            let elapsed = clock().saturating_duration_since(began).as_nanos().max(1);
            let estimate = (window.bytes as u128 * TARGET.as_nanos() / elapsed).min(MAX as u128) as usize;
            self.size = (estimate / MIB * MIB).clamp(MIB, MAX);
        }
        Ok(Some(window.offset + window.bytes as u64))
    }
}

#[cfg(test)]
#[path = "../../test/writeback.rs"]
mod tests;
