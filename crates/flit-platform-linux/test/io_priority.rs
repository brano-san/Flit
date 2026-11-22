use super::*;
use flit_core::digest::HashKind;
use flit_core::FsPath;
use flit_platform::{Fs, Job};
use std::cell::Cell;

thread_local! {
    static PRIORITY: Cell<Option<libc::c_int>> = const { Cell::new(None) };
    static DENIED: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn current() -> Option<libc::c_int> {
    PRIORITY.get()
}

pub(super) fn set(value: libc::c_int) -> bool {
    if DENIED.get() {
        return false;
    }
    PRIORITY.set(Some(value));
    true
}

#[test]
fn background_hash_runs_at_idle_and_restores_the_worker() {
    let tmp = crate::temp::Temp::new("idle-io");
    let file = tmp.path().join("abc");
    std::fs::write(&file, b"abc").unwrap();
    let path = FsPath::parse(file.to_str().unwrap()).unwrap();
    let job = Job::new(Default::default(), Box::new(|_| assert_eq!(current(), Some(IDLE))), Box::new(|_| {}));
    PRIORITY.set(Some(0));
    assert_eq!(crate::LinuxFs.hash(&path, &[HashKind::Crc32], &job).unwrap(), ["352441c2"]);
    assert_eq!(PRIORITY.replace(None), Some(0));
}

#[test]
fn denied_priority_changes_keep_the_original_priority() {
    PRIORITY.set(Some(0));
    DENIED.set(true);
    let guard = IdleIo::enter();
    assert_eq!(current(), Some(0));
    drop(guard);
    DENIED.set(false);
    assert_eq!(PRIORITY.replace(None), Some(0));
}
