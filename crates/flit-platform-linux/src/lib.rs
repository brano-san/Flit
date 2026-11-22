#![cfg(target_os = "linux")]

pub mod agent;
pub mod desktop;
mod busy;
mod fs;
mod inotify;
pub mod mounts;
mod ops;
pub mod sock;
mod tags;
mod trash;
mod walk;

#[cfg(test)]
#[path = "../test/temp.rs"]
mod temp;

pub use fs::{host, stat_path, xattr, xattr_names, LinuxFs};
pub use inotify::{Inotify, Notice, Notify};
pub use ops::run as run_op;
pub use tags::{file_key, merged_tag_names, resolve_files, sync_tag_names};
pub use walk::{indexed, WalkIndex};
pub use trash::{percent_decode, percent_encode, trash_time, Trash};

use flit_platform::{IndexPlatform, IndexSource, OnClient, PlatformError, Service};

pub struct LinuxIndex;

impl IndexPlatform for LinuxIndex {
    type Source = WalkIndex;
    type Walk = WalkIndex;
    type Service = Systemd;

    fn walk_source(_: &[String]) -> Result<Self::Walk, PlatformError> {
        WalkIndex::new()
    }

    fn utc_offset_minutes() -> i32 {
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
            return 0;
        }
        (tm.tm_gmtoff / 60) as i32
    }

    fn state_dir() -> Option<std::path::PathBuf> {
        let absolute = |v: &str| std::env::var_os(v).map(std::path::PathBuf::from).filter(|p| p.is_absolute());
        Some(absolute("XDG_STATE_HOME").or_else(|| Some(absolute("HOME")?.join(".local/state")))?.join("flit"))
    }
}

pub struct Systemd;

impl Service for Systemd {
    type Conn = sock::UnixConn;
    fn endpoint() -> String {
        sock::endpoint().map_or_else(|| format!("$XDG_RUNTIME_DIR/{}", sock::SOCKET), |p| p.display().to_string())
    }
    fn run(body: fn() -> Result<(), PlatformError>) -> Result<(), PlatformError> {
        body()
    }
    fn connect() -> Result<sock::UnixConn, PlatformError> {
        let path = sock::endpoint().ok_or_else(|| PlatformError::new(-1, "XDG_RUNTIME_DIR is not set"))?;
        sock::connect_at(&path, sock::uid(), sock::CONNECT_TIMEOUT)
    }
    fn listen(max_clients: usize, on_client: OnClient<sock::UnixConn>) -> Result<(), PlatformError> {
        let listener = match sock::activated() {
            Some(l) => l,
            None => sock::bind_at(&sock::endpoint().ok_or_else(|| PlatformError::new(-1, "XDG_RUNTIME_DIR is not set"))?, sock::uid())?,
        };
        sock::serve_on(listener, sock::uid(), max_clients, on_client)
    }
}
