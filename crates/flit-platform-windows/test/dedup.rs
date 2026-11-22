use flit_core::FsPath;
use flit_core::provider::{FileOp, OpReport, Resolve};
use flit_core::tags::FileKey;
use flit_core::undo::{Step, inverse};
use flit_platform::{FileOps, Job, OpOutcome, PlatformError, std_path};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Recovery(FsPath, bool);
impl FileOps for Recovery {
    fn verify_dedup_data(&mut self, path: &FsPath) -> Result<(), PlatformError> {
        crate::fileops::verify_dedup_data(path)
    }

    fn file_identity(&mut self, path: &FsPath) -> Option<FileKey> { crate::tags::file_key(path) }

    fn keep_dedup_backup(&mut self, path: &FsPath, original: &FsPath) -> Option<FsPath> {
        assert!(std_path(original).is_file());
        assert_ne!(self.file_identity(path), self.file_identity(original));
        if self.1 { return None; }
        let copy = self.0.join(b"recycle-record");
        fs::rename(std_path(path), std_path(&copy)).ok()?;
        Some(copy)
    }

    fn rename_local_noreplace(&mut self, from: &FsPath, to: &FsPath) -> Result<(), PlatformError> {
        crate::fileops::rename_local_noreplace(from, to)
    }

    fn file_op(&mut self, op: &FileOp, _: &Job) -> OpOutcome {
        let FileOp::Purge(pairs) = op else { panic!("unexpected operation {op:?}") };
        for (from, to) in pairs {
            assert!(std_path(to).is_file());
            assert_eq!(self.file_identity(from), self.file_identity(to));
            fs::remove_file(std_path(from)).unwrap();
        }
        OpOutcome { items: pairs.clone(), error: None }
    }

    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> { unreachable!() }
    fn clear_clipboard(&mut self) { unreachable!() }
}

struct Temp(FsPath);
impl Temp {
    fn new() -> Self {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("flit-dedup-{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        let path = path.canonicalize().unwrap();
        use std::os::windows::ffi::OsStrExt;
        Self(FsPath::from_utf16(&path.as_os_str().encode_wide().collect::<Vec<_>>()).unwrap())
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = fs::remove_dir_all(std_path(&self.0)); }
}

fn replace_and_undo(target_name: &[u8]) {
    let temp = Temp::new();
    let keep = temp.0.join(b"keep.txt");
    let target = temp.0.join(target_name);
    let bytes = b"same bytes";
    for path in [&keep, &target] { fs::write(std_path(path), bytes).unwrap(); }
    let mut recovery = Recovery(temp.0.clone(), false);
    let saved = recovery.file_identity(&target).unwrap();
    let job = Job::detached(Resolve::Skip);
    let op = FileOp::Dedup { keep: keep.clone(), candidates: vec![target.clone()], backups: Vec::new() };
    let finished = flit_platform::dedup::run(&mut recovery, &op, &job).unwrap();
    assert!(finished.error.is_none(), "{:?}", finished.error);
    assert_eq!(recovery.file_identity(&keep), recovery.file_identity(&target));
    let report = OpReport { op: job.take_performed().unwrap(), items: finished.items };
    let Some(Step::Run(undo)) = inverse(&report) else { panic!("no Undo for the replacement") };
    let finished = flit_platform::dedup::run(&mut recovery, &undo, &Job::detached(Resolve::Skip)).unwrap();
    assert!(finished.error.is_none(), "{:?}", finished.error);
    assert_eq!(recovery.file_identity(&target), Some(saved));
    assert_eq!(fs::read(std_path(&target)).unwrap(), bytes);
}

#[test]
fn replacing_and_undoing_duplicates_restores_the_original_file_identity() {
    replace_and_undo(b"duplicate.txt");
}

#[test]
fn undo_preserves_a_duplicate_named_like_the_temporary_link() {
    replace_and_undo(b".FLIT-LINKED-BEFORE-UNDO");
}

#[test]
fn publication_leaves_a_competing_file_and_the_staged_data_intact() {
    let temp = Temp::new();
    let source = temp.0.join(b"staged.txt");
    let dest = temp.0.join(b"existing.txt");
    fs::write(std_path(&source), b"staged bytes").unwrap();
    fs::write(std_path(&dest), b"other program's bytes").unwrap();
    assert!(crate::fileops::rename_local_noreplace(&source, &dest).is_err());
    assert_eq!(fs::read(std_path(&source)).unwrap(), b"staged bytes");
    assert_eq!(fs::read(std_path(&dest)).unwrap(), b"other program's bytes");
}

#[test]
fn equal_length_files_with_different_bytes_are_not_replaced() {
    let temp = Temp::new();
    let keep = temp.0.join(b"keep.txt");
    let target = temp.0.join(b"different.txt");
    fs::write(std_path(&keep), b"abc").unwrap();
    fs::write(std_path(&target), b"def").unwrap();
    let mut recovery = Recovery(temp.0.clone(), false);
    let saved = recovery.file_identity(&target).unwrap();
    let job = Job::detached(Resolve::Skip);
    let op = FileOp::Dedup { keep, candidates: vec![target.clone()], backups: Vec::new() };
    let finished = flit_platform::dedup::run(&mut recovery, &op, &job).unwrap();
    assert!(finished.error.is_some() && finished.items.is_empty());
    assert_eq!(recovery.file_identity(&target), Some(saved));
    assert_eq!(fs::read(std_path(&target)).unwrap(), b"def");
    assert_eq!(fs::read_dir(std_path(&temp.0)).unwrap().count(), 2);
}

#[test]
fn replacement_rejects_an_unknown_stream_on_the_kept_file_by_name() {
    let temp = Temp::new();
    let keep = temp.0.join(b"keep.txt");
    let target = temp.0.join(b"duplicate.txt");
    let stream = temp.0.join(b"keep.txt:details");
    for item in [&keep, &target] { fs::write(std_path(item), b"same").unwrap(); }
    fs::write(std_path(&stream), b"metadata").unwrap();
    let mut recovery = Recovery(temp.0.clone(), false);
    let first = recovery.file_identity(&target);
    let op = FileOp::Dedup { keep, candidates: vec![target.clone()], backups: Vec::new() };
    let finished = flit_platform::dedup::run(&mut recovery, &op, &Job::detached(Resolve::Skip)).unwrap();
    assert!(finished.error.unwrap().to_string().contains("details"));
    assert!(finished.items.is_empty());
    assert_eq!(recovery.file_identity(&target), first);
    assert_eq!(fs::read(std_path(&stream)).unwrap(), b"metadata");
}

#[test]
fn replacement_accepts_download_marks_and_round_trips_the_targets_streams() {
    let temp = Temp::new();
    let keep = temp.0.join(b"keep.txt");
    let target = temp.0.join(b"duplicate.txt");
    let stream = temp.0.join(b"duplicate.txt:details");
    for item in [&keep, &target] { fs::write(std_path(item), b"same").unwrap(); }
    fs::write(std_path(&temp.0.join(b"keep.txt:Zone.Identifier")), b"[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();
    fs::write(std_path(&stream), b"metadata").unwrap();
    let mut recovery = Recovery(temp.0.clone(), false);
    let job = Job::detached(Resolve::Skip);
    let op = FileOp::Dedup { keep, candidates: vec![target.clone()], backups: Vec::new() };
    let finished = flit_platform::dedup::run(&mut recovery, &op, &job).unwrap();
    assert!(finished.error.is_none(), "{:?}", finished.error);
    assert!(!std_path(&stream).exists());
    let report = OpReport { op: job.take_performed().unwrap(), items: finished.items };
    let Some(Step::Run(undo)) = inverse(&report) else { panic!("no Undo for the replacement") };
    let finished = flit_platform::dedup::run(&mut recovery, &undo, &Job::detached(Resolve::Skip)).unwrap();
    assert!(finished.error.is_none(), "{:?}", finished.error);
    assert_eq!(fs::read(std_path(&stream)).unwrap(), b"metadata");
}

#[test]
fn a_failed_backup_rolls_back_without_losing_the_target_name_or_identity() {
    let temp = Temp::new();
    let keep = temp.0.join(b"keep.txt");
    let target = temp.0.join(b"original.txt");
    for item in [&keep, &target] { fs::write(std_path(item), b"equal").unwrap(); }
    let mut recovery = Recovery(temp.0.clone(), true);
    let original = recovery.file_identity(&target);
    let op = FileOp::Dedup { keep, candidates: vec![target.clone()], backups: Vec::new() };
    let outcome = flit_platform::dedup::run(&mut recovery, &op, &Job::detached(Resolve::Skip)).unwrap();
    assert!(outcome.error.is_some());
    assert!(outcome.items.is_empty());
    assert_eq!(recovery.file_identity(&target), original);
    assert_eq!(fs::read(std_path(&target)).unwrap(), b"equal");
    assert_eq!(fs::read_dir(std_path(&temp.0)).unwrap().count(), 2);
}
