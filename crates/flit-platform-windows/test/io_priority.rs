use super::*;
use crate::fs::NtFs;
use flit_core::digest::HashKind;
use flit_core::provider::{Resolve, Totals};
use flit_core::{FsPath, ListingBuilder};
use flit_platform::{Fs, Job};
use std::cell::Cell;

thread_local! {
    static HINTS: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn apply(handle: HANDLE, hint: &AlignedHint) {
    assert!(!handle.is_invalid());
    assert_eq!(hint.0.PriorityHint.0, IoPriorityHintVeryLow.0);
    assert_eq!(std::ptr::from_ref(hint).addr() % 8, 0);
    HINTS.set(HINTS.get() + 1);
}

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = crate::test_temp().join(format!("flit-io-priority-{name}-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> FsPath {
        FsPath::parse(self.0.to_str().unwrap()).unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn hashes_use_low_priority_handles_and_folder_properties_keep_normal_priority() {
    let dir = TempDir::new("hash");
    let file = dir.0.join("item");
    std::fs::write(&file, b"abc").unwrap();
    let path = FsPath::parse(file.to_str().unwrap()).unwrap();
    let hash = NtFs.hash(&path, &[HashKind::Crc32], &Job::detached(Resolve::Skip)).unwrap();
    assert_eq!(hash, ["352441c2"]);
    assert_eq!(HINTS.replace(0), 1);
    let root = path.parent().unwrap();
    let totals = NtFs.measure(&[(root, None)], &|| true, &mut |_: &Totals| {});
    assert_eq!(totals.size, 3);
    assert_eq!(HINTS.replace(0), 0);
}

#[test]
fn opening_a_visible_folder_keeps_normal_io_priority() {
    let dir = TempDir::new("visible");
    std::fs::write(dir.0.join("entry"), b"").unwrap();
    let mut each = |_: &ListingBuilder<'_>| true;
    let mut listing = ListingBuilder::new(flit_core::sort::CaseMode::Insensitive, &mut each);
    NtFs.read_dir(&dir.path(), &mut listing).unwrap();
    assert_eq!(listing.listing.len(), 1);
    assert_eq!(HINTS.replace(0), 0);
}
