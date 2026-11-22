use super::*;
use flit_core::undo::Step;
use flit_platform::{replay, FileOps, OpOutcome};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct TransferFixture {
    dir: PathBuf,
    root: FsPath,
    can_recycle: bool,
    locked_backup: Option<FsPath>,
    shell_only: bool,
    shell_refuses: bool,
    shell_attempts: std::cell::Cell<usize>,
}

impl TransferFixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = crate::test_temp().join(format!("flit-transfer-history-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let root = dir.join(r"$Recycle.Bin\test");
        std::fs::create_dir_all(&root).unwrap();
        for folder in ["source", "destination"] {
            std::fs::create_dir(dir.join(folder)).unwrap();
        }
        Self { dir, root: FsPath::parse(root.to_str().unwrap()).unwrap(), can_recycle: true, locked_backup: None, shell_only: false, shell_refuses: false, shell_attempts: Default::default() }
    }

    fn path(&self, name: &str) -> FsPath {
        FsPath::parse(self.dir.join(name).to_str().unwrap()).unwrap()
    }

    fn write(&self, name: &str, bytes: &[u8]) -> FsPath {
        std::fs::write(self.dir.join(name), bytes).unwrap();
        self.path(name)
    }

    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.join(name)).unwrap()
    }

    fn put_in_bin(&self, path: &FsPath) -> Option<FsPath> {
        if !self.can_recycle {
            return None;
        }
        static SEED: AtomicU64 = AtomicU64::new(1);
        let mut seed = SEED.fetch_add(1, Ordering::Relaxed);
        let mut budget = u64::MAX;
        crate::recycle::recycle_payload(&self.root, path, path, &mut budget, &mut seed, true)
    }

    fn apply(&mut self, op: FileOp) -> flit_core::provider::OpReport {
        let job = Job::detached(Resolve::Replace);
        let out = self.file_op(&op, &job);
        assert!(out.error.is_none(), "{:?}", out.error);
        replay::report(op, out.items, self.undo_fs(), &job)
    }

    fn execute(&mut self, step: Step) -> (OpOutcome, Option<Step>) {
        let Step::Checked(replay) = step else { panic!("expected a checked step, got {step:?}") };
        let job = Job::new(flit_core::provider::OpCtl::checked(replay.clone()), Box::new(|_| {}), Box::new(|_| {}));
        let fs = self.undo_fs();
        replay::run(self, &replay, fs, &job)
    }
}

impl Drop for TransferFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Recycle for TransferFixture {
    fn direct(&self, path: &FsPath) -> Option<FsPath> {
        if self.shell_only { None } else { self.put_in_bin(path) }
    }

    fn shell(&self, path: &FsPath, _: &Job) -> Result<FsPath, flit_platform::PlatformError> {
        self.shell_attempts.set(self.shell_attempts.get() + 1);
        if self.shell_refuses {
            return Err(flit_platform::PlatformError::new(-1, S::ErrorsEngineDeletionCancelled.en()));
        }
        self.put_in_bin(path).ok_or_else(|| flit_platform::PlatformError::new(-1, S::ErrorsOpsUndoFailed.en()))
    }

    fn restore(&self, backup: &FsPath, original: &FsPath) -> bool {
        self.locked_backup.as_ref() != Some(backup) && crate::fileops::restore_checked(backup, original)
    }
}

impl FileOps for TransferFixture {
    fn file_op(&mut self, op: &FileOp, job: &Job) -> OpOutcome {
        crate::fileops::checked_op(op, job, self)
    }

    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        unreachable!()
    }

    fn clear_clipboard(&mut self) {}

    fn undo_fs(&self) -> Option<replay::FileSystem> {
        Some(replay::FileSystem { stamp: undo_stamp, children: undo_children })
    }

    fn can_restore(&self, backup: &FsPath, original: &FsPath) -> bool {
        crate::recycle::is_original(backup, original)
    }
}

#[test]
fn copy_and_move_replacements_restore_both_versions_and_redo() {
    for moving in [false, true] {
        let mut t = TransferFixture::new();
        let from = t.write(r"source\a.bin", b"new\x00bytes");
        let to = t.write(r"destination\a.bin", b"old\xFFbytes");
        let dest = to.parent().unwrap();
        let op = if moving { FileOp::Move { items: vec![from], dest } } else { FileOp::Copy { items: vec![from], dest } };
        let report = t.apply(op);
        assert_eq!(t.read(r"destination\a.bin"), b"new\x00bytes");
        assert_eq!(report.undo.replaced.len(), 1);

        let (undone, redo) = t.execute(flit_core::undo::inverse(&report).unwrap());
        assert!(undone.error.is_none(), "{:?}", undone.error);
        assert_eq!(t.read(r"source\a.bin"), b"new\x00bytes");
        assert_eq!(t.read(r"destination\a.bin"), b"old\xFFbytes");

        let (redone, _) = t.execute(redo.unwrap());
        assert!(redone.error.is_none(), "{:?}", redone.error);
        assert_eq!(t.read(r"destination\a.bin"), b"new\x00bytes");
        assert_eq!(t.dir.join(r"source\a.bin").exists(), !moving);
    }
}

#[test]
fn unavailable_recycling_disables_undo_for_the_entire_copy() {
    let mut t = TransferFixture::new();
    t.can_recycle = false;
    let old = t.write(r"source\existing", b"incoming");
    let new = t.write(r"source\other", b"unrelated");
    t.write(r"destination\existing", b"outgoing");
    let report = t.apply(FileOp::Copy { items: vec![old, new], dest: t.path("destination") });
    assert!(matches!(flit_core::undo::inverse(&report), Some(Step::Refuse(_))));
    assert_eq!(t.read(r"destination\existing"), b"incoming");
    assert_eq!(t.read(r"destination\other"), b"unrelated");
}

#[test]
fn missing_recycle_metadata_blocks_undo_before_it_touches_the_replacement() {
    let mut t = TransferFixture::new();
    let from = t.write(r"source\file", b"new");
    t.write(r"destination\file", b"old");
    let report = t.apply(FileOp::Copy { items: vec![from], dest: t.path("destination") });
    let backup = report.undo.replaced[0].1.as_ref().unwrap();
    let info = crate::recycle::info_file(backup).unwrap();
    std::fs::remove_file(crate::fileops::std_path(&info)).unwrap();
    let (result, _) = t.execute(flit_core::undo::inverse(&report).unwrap());
    assert!(result.error.is_some());
    assert!(result.items.is_empty());
    assert_eq!(t.read(r"destination\file"), b"new");
    assert_eq!(std::fs::read(crate::fileops::std_path(backup)).unwrap(), b"old");
}

#[test]
fn a_failed_restore_puts_the_new_file_back_and_keeps_undo_retryable() {
    let mut t = TransferFixture::new();
    let from = t.write(r"source\file", b"new");
    t.write(r"destination\file", b"old");
    let report = t.apply(FileOp::Copy { items: vec![from], dest: t.path("destination") });
    t.locked_backup = report.undo.replaced[0].1.clone();
    let step = flit_core::undo::inverse(&report).unwrap();
    let (result, next) = t.execute(step.clone());
    assert_eq!(result.error.unwrap().message, S::ErrorsOpsUndoFailed.en());
    assert!(result.items.is_empty() && next.is_none());
    assert_eq!(t.read(r"destination\file"), b"new");
    t.locked_backup = None;
    let (result, _) = t.execute(step);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(t.read(r"destination\file"), b"old");
}

#[test]
fn one_modified_copy_blocks_undo_of_all_items() {
    let mut t = TransferFixture::new();
    let a = t.write(r"source\a", b"one");
    let b = t.write(r"source\b", b"two");
    let report = t.apply(FileOp::Copy { items: vec![a, b], dest: t.path("destination") });
    t.write(r"destination\b", b"changed since copying");
    let (out, _) = t.execute(flit_core::undo::inverse(&report).unwrap());
    assert!(out.items.is_empty());
    assert!(out.error.unwrap().message.contains(r"destination\b"));
    assert_eq!(t.read(r"destination\a"), b"one");
    assert_eq!(t.read(r"destination\b"), b"changed since copying");
}

#[test]
fn a_completed_part_of_a_cancelled_operation_can_be_undone() {
    let mut t = TransferFixture::new();
    let from = t.write(r"source\file", b"new");
    let op = FileOp::Copy { items: vec![from], dest: t.path("destination") };
    let job = Job::detached(Resolve::Replace);
    let result = t.file_op(&op, &job);
    assert!(result.error.is_none());
    job.ctl().cancel();
    let report = replay::report(op, result.items, Some(undo_fs()), &job);
    let (result, _) = t.execute(flit_core::undo::inverse(&report).unwrap());
    assert!(result.error.is_none());
    assert!(!exists(&t.path(r"destination\file")));
    assert_eq!(t.read(r"source\file"), b"new");
}

#[test]
fn undo_trash_recreates_an_original_directory_that_is_gone() {
    let mut t = TransferFixture::new();
    let from = t.write(r"source\file", b"recover");
    let report = t.apply(FileOp::Trash(vec![from]));
    std::fs::remove_dir(t.dir.join("source")).unwrap();
    let (result, _) = t.execute(flit_core::undo::inverse(&report).unwrap());
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(t.read(r"source\file"), b"recover");
}

#[test]
fn an_occupied_move_origin_blocks_undo_and_can_be_retried() {
    let mut t = TransferFixture::new();
    let a = t.write(r"source\a", b"moving");
    let report = t.apply(FileOp::Move { items: vec![a], dest: t.path("destination") });
    t.write(r"source\a", b"unrelated");
    let step = flit_core::undo::inverse(&report).unwrap();
    let (out, _) = t.execute(step.clone());
    assert!(out.items.is_empty());
    assert!(out.error.unwrap().message.contains(r"source\a"));
    assert_eq!(t.read(r"source\a"), b"unrelated");
    assert_eq!(t.read(r"destination\a"), b"moving");
    std::fs::remove_file(t.dir.join(r"source\a")).unwrap();
    let (out, _) = t.execute(step);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(t.read(r"source\a"), b"moving");
    assert!(!t.dir.join(r"destination\a").exists());
}

#[test]
fn grouped_undo_checks_descendants_even_when_directory_timestamps_drift() {
    let mut t = TransferFixture::new();
    let contents = t.dir.join(r"source\tree");
    std::fs::create_dir(&contents).unwrap();
    t.write(r"source\tree\original", b"moving");
    let report = t.apply(FileOp::Move { items: vec![t.path(r"source\tree")], dest: t.path("destination") });
    let Some(Step::Checked(mut plan)) = flit_core::undo::inverse(&report) else { panic!("expected a checked step"); };
    plan.recheck_tree = true;
    let unexpected = t.write(r"destination\tree\foreign", b"preserve");
    std::fs::File::options().write(true).open(crate::fileops::std_path(&unexpected)).unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(1)).unwrap();
    let (refused, _) = t.execute(Step::Checked(plan.clone()));
    assert!(refused.error.is_some());
    assert_eq!(t.read(r"destination\tree\foreign"), b"preserve");
    std::fs::remove_file(crate::fileops::std_path(&unexpected)).unwrap();
    let (restored, _) = t.execute(Step::Checked(plan));
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(t.read(r"source\tree\original"), b"moving");
}
#[test]
fn a_staged_delete_disappears_before_removal_and_is_resumed_after_a_crash() {
    let t = TransferFixture::new();
    let state = t.dir.join("state");
    let delete = t.path(r"source\remove");
    std::fs::create_dir(crate::fileops::std_path(&delete)).unwrap();
    t.write(r"source\remove\already-removed", b"first");
    t.write(r"source\remove\remaining", b"second");
    let foreign = t.write(r"source\.flit-deleting-foreign", b"keep");

    let (staged, crashed_record) = flit_platform::pending::stage_delete(&state, &delete, undo_stamp(&delete).unwrap().0, false, |from, to| {
        move_file(from, to, MOVE_FILE_FLAGS(0)).map_err(io_error)
    }).unwrap();
    assert!(!exists(&delete) && exists(&staged));
    crashed_record.inventory(&staged, undo_fs(), || false).unwrap();
    crashed_record.begin_delete().unwrap();
    drop(crashed_record);
    recover_pending(&state, |_| true);
    assert!(exists(&staged));

    let child = staged.join(b"already-removed");
    delete_file(&child).unwrap();
    let foreign_child = staged.join(b"unregistered-child");
    std::fs::write(crate::fileops::std_path(&foreign_child), b"preserve").unwrap();
    recover_pending(&state, |_| false);
    assert!(exists(&foreign_child));
    delete_file(&foreign_child).unwrap();
    recover_pending(&state, |_| false);
    assert!(!exists(&staged));
    assert_eq!(std::fs::read(crate::fileops::std_path(&foreign)).unwrap(), b"keep");
}

#[test]
fn recovery_only_removes_registered_partial_stages_and_preserves_a_displaced_file() {
    let t = TransferFixture::new();
    let state = t.dir.join("state");
    for name in ["owned.flit-partial", "foreign.flit-partial", "rollback.flit-partial"] {
        std::fs::create_dir(t.dir.join(name)).unwrap();
        t.write(&format!(r"{name}\new"), b"payload");
    }
    t.write(r"rollback.flit-partial\replaced", b"original files remain recoverable");
    for name in ["owned.flit-partial", "rollback.flit-partial"] {
        let path = t.path(name);
        let _crashed_record = flit_platform::pending::partial(&state, &path, undo_stamp(&path).unwrap().0).unwrap();
    }
    recover_pending(&state, |_| false);
    assert!(!exists(&t.path("owned.flit-partial")));
    assert!(exists(&t.path(r"foreign.flit-partial\new")));
    assert_eq!(t.read(r"rollback.flit-partial\replaced"), b"original files remain recoverable");
}

#[test]
fn a_failed_or_cancelled_cross_volume_replacement_preserves_both_sides() {
    for cancelled in [false, true] {
        let t = TransferFixture::new();
        let from = t.write(r"source\item", b"whole source");
        let to = t.write(r"destination\item", b"old destination");
        let job = Job::detached(Resolve::Replace);
        let mut engine = Engine { recycle: &t, pending_state: Some(t.dir.join("state")), ..Engine::new(&job) };
        let error = engine.replace_staged(&from, &to, Mode::Across, |staged| {
            std::fs::write(crate::fileops::std_path(staged), b"partial").unwrap();
            if cancelled {
                job.ctl().cancel();
                Ok(())
            } else {
                Err(ERROR_DISK_FULL.to_hresult().into())
            }
        }).unwrap_err();
        assert_eq!(code(&error), if cancelled { ERROR_CANCELLED } else { ERROR_DISK_FULL });
        assert_eq!(t.read(r"source\item"), b"whole source");
        assert_eq!(t.read(r"destination\item"), b"old destination");
        assert!(job.take_replaced().is_empty());
        assert_eq!(std::fs::read_dir(t.dir.join("destination")).unwrap().count(), 1);
    }
}

#[test]
fn cancelled_delete_restores_all_unprocessed_names_and_closes_the_journal() {
    let t = TransferFixture::new();
    let state = t.dir.join("state");
    let paths: Vec<_> = (0..10).map(|index| {
        let path = t.path(&format!(r"source\item-{index}"));
        std::fs::create_dir(crate::fileops::std_path(&path)).unwrap();
        path
    }).collect();
    let stage = LinkStage::new(&t.path(r"source\output"), LinkKind::Hard, false, Some(&state)).unwrap();
    t.write(r"source\.flit-stage-user.txt", b"ordinary file");
    let ctl = flit_core::provider::OpCtl::default();
    let cancel = ctl.clone();
    let directory = t.path("source");
    let job = Job::new(ctl, Box::new(move |progress| {
        if progress.stage == Stage::Running && progress.items == 0 {
            let stages = undo_children(&directory).unwrap();
            assert_eq!(stages.len(), 12);
            assert!(stages.iter().filter(|path| flit_platform::pending::is_stage_name(path.file_name())).all(|path| {
                let attributes = unsafe { GetFileAttributesW(PCWSTR(wide(path).as_ptr())) };
                attributes & FILE_ATTRIBUTE_HIDDEN.0 != 0
            }));
            std::thread::sleep(std::time::Duration::from_millis(40));
        } else if progress.items == 1 {
            cancel.cancel();
        }
    }), Box::new(|_| {}));
    let mut engine = Engine::new(&job);
    assert!(engine.delete(&paths, Some(&state)).is_none());
    drop(stage);
    assert_eq!(engine.done, vec![(paths[0].clone(), paths[0].clone())]);
    assert!(!exists(&paths[0]));
    assert!(paths[1..].iter().all(exists));
    assert_eq!(std::fs::read_dir(state.join("pending-deletes")).unwrap().count(), 0);
    recover_pending(&state, |_| false);
    assert!(paths[1..].iter().all(exists));
}

#[test]
fn a_folder_listing_excludes_both_kinds_of_operation_stages() {
    let t = TransferFixture::new();
    let directory = t.path("source");
    let state = t.dir.join("state");
    let original = t.write(r"source\victim", b"delete stage");
    let (staged, record) = flit_platform::pending::stage_delete(&state, &original, undo_stamp(&original).unwrap().0, false, |from, to| {
        move_file(from, to, MOVE_FILE_FLAGS(0)).map_err(io_error)
    }).unwrap();
    let stage = LinkStage::new(&t.path(r"source\output"), LinkKind::Hard, false, Some(&state)).unwrap();
    t.write(r"source\.flit-stage-project.txt", b"regular file");
    let mut keep = |_: &ListingBuilder<'_>| true;
    let mut listing = ListingBuilder::new(CaseMode::Sensitive, &mut keep);
    NtFs.read_dir(&directory, &mut listing).unwrap();
    assert_eq!(listing.listing.len(), 1);
    assert_eq!(listing.listing.name(0), b".flit-stage-project.txt");
    assert!(exists(&staged) && exists(&stage.dir));
    move_file(&staged, &original, MOVE_FILE_FLAGS(0)).unwrap();
    record.finish();
}

#[test]
fn a_cancelled_delete_with_an_occupied_name_is_visible_and_restored_when_free() {
    let t = TransferFixture::new();
    let state = t.dir.join("state");
    let original = t.write(r"source\item", b"keep this");
    set_hidden(&original, true).unwrap();
    let path = crate::fileops::std_path(&original);
    let ctl = flit_core::provider::OpCtl::default();
    let cancel = ctl.clone();
    let job = Job::new(ctl, Box::new(move |progress| {
        if progress.stage == Stage::Running {
            std::fs::write(&path, b"occupant").unwrap();
            cancel.cancel();
        }
    }), Box::new(|_| {}));
    let mut engine = Engine::new(&job);
    assert!(engine.delete(std::slice::from_ref(&original), Some(&state)).is_none());
    let staged = undo_children(&t.path("source")).unwrap().into_iter()
        .find(|path| flit_platform::pending::is_delete_name(path.file_name())).unwrap();
    assert!(engine.done.is_empty() && engine.error().unwrap().message.contains(staged.display().as_ref()));
    assert_visible_delete_stage(&t, &staged);
    assert_eq!(std::fs::read_dir(state.join("pending-deletes")).unwrap().count(), 1);

    recover_pending(&state, |_| false);
    assert_eq!(t.read(r"source\item"), b"occupant");
    assert_eq!(std::fs::read(crate::fileops::std_path(&staged)).unwrap(), b"keep this");
    assert_visible_delete_stage(&t, &staged);
    delete_file(&original).unwrap();
    recover_pending(&state, |_| false);
    assert_eq!(t.read(r"source\item"), b"keep this");
    assert_ne!(stat(&original).unwrap().attrs & attr::HIDDEN, 0);
    assert!(!exists(&staged));
    assert_eq!(std::fs::read_dir(state.join("pending-deletes")).unwrap().count(), 0);
}

#[test]
fn a_changed_stage_identity_is_kept_visible_and_is_never_restored_as_the_original() {
    let t = TransferFixture::new();
    let state = t.dir.join("state");
    let original = t.write(r"source\saved", b"original");
    let detour = t.path(r"source\different-name");
    let (dir, detour_path) = (t.path("source"), detour.clone());
    let job = Job::new(flit_core::provider::OpCtl::default(), Box::new(move |progress| {
        if progress.stage == Stage::Running {
            let staged = undo_children(&dir).unwrap().into_iter().next().unwrap();
            move_file(&staged, &detour_path, MOVE_FILE_FLAGS(0)).unwrap();
            std::fs::write(crate::fileops::std_path(&staged), b"foreign").unwrap();
            set_hidden(&staged, true).unwrap();
        }
    }), Box::new(|_| {}));
    let mut engine = Engine::new(&job);
    engine.delete(std::slice::from_ref(&original), Some(&state));
    let staged = undo_children(&t.path("source")).unwrap().into_iter()
        .find(|path| flit_platform::pending::is_delete_name(path.file_name())).unwrap();
    assert!(engine.error().unwrap().message.contains(staged.display().as_ref()));
    assert_visible_delete_stage(&t, &staged);
    recover_pending(&state, |_| false);
    assert!(!exists(&original));
    assert_eq!(std::fs::read(crate::fileops::std_path(&staged)).unwrap(), b"foreign");
    assert_eq!(std::fs::read_dir(state.join("pending-deletes")).unwrap().count(), 1);

    delete_file(&staged).unwrap();
    move_file(&detour, &staged, MOVE_FILE_FLAGS(0)).unwrap();
    recover_pending(&state, |_| false);
    assert_eq!(t.read(r"source\saved"), b"original");
}

#[test]
fn a_directory_with_an_unwritable_inventory_is_restored_without_deleting_its_children() {
    let t = TransferFixture::new();
    let state = t.dir.join("state");
    let original = t.path(r"source\tree");
    std::fs::create_dir(crate::fileops::std_path(&original)).unwrap();
    t.write(r"source\tree\child", b"keep");
    let logs = state.join("pending-deletes");
    let sabotage = logs.clone();
    let job = Job::new(flit_core::provider::OpCtl::default(), Box::new(move |progress| {
        if progress.stage == Stage::Running {
            let log = std::fs::read_dir(&sabotage).unwrap().next().unwrap().unwrap().path();
            std::fs::write(log.with_extension("objects"), b"unusable inventory").unwrap();
        }
    }), Box::new(|_| {}));
    let mut engine = Engine::new(&job);
    engine.delete(std::slice::from_ref(&original), Some(&state));
    assert!(engine.done.is_empty() && engine.error().is_some());
    assert_eq!(t.read(r"source\tree\child"), b"keep");
    assert_eq!(std::fs::read_dir(logs).unwrap().count(), 0);
}

#[test]
fn recovery_restores_the_name_of_a_directory_when_inventory_never_finished() {
    let t = TransferFixture::new();
    let state = t.dir.join("state");
    let original = t.path(r"source\tree");
    std::fs::create_dir(crate::fileops::std_path(&original)).unwrap();
    t.write(r"source\tree\child", b"keep");
    let (staged, record) = flit_platform::pending::stage_delete(&state, &original, undo_stamp(&original).unwrap().0, false, |from, to| {
        move_file(from, to, MOVE_FILE_FLAGS(0)).map_err(io_error)
    }).unwrap();
    set_hidden(&staged, true).unwrap();
    assert!(record.inventory(&staged, undo_fs(), || true).is_err());
    drop(record);

    recover_pending(&state, |_| false);
    assert_eq!(t.read(r"source\tree\child"), b"keep");
    assert_eq!(stat(&original).unwrap().attrs & attr::HIDDEN, 0);
    assert!(!exists(&staged));
    assert_eq!(std::fs::read_dir(state.join("pending-deletes")).unwrap().count(), 0);
}

#[test]
fn a_locked_delete_marker_does_not_rearm_cancelled_cleanup_on_restart() {
    use std::os::windows::fs::OpenOptionsExt;

    let t = TransferFixture::new();
    let state = t.dir.join("state");
    let original = t.write(r"source\file", b"keep");
    let (staged, record) = flit_platform::pending::stage_delete(&state, &original, undo_stamp(&original).unwrap().0, false, |from, to| {
        move_file(from, to, MOVE_FILE_FLAGS(0)).map_err(io_error)
    }).unwrap();
    record.begin_delete().unwrap();
    let marker = std::fs::read_dir(state.join("pending-deletes")).unwrap().map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "delete")).unwrap();
    let lock = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ.0).open(&marker).unwrap();
    record.keep();
    assert!(marker.exists());
    drop(lock);
    drop(record);

    recover_pending(&state, |_| false);
    assert_eq!(t.read(r"source\file"), b"keep");
    assert!(!exists(&staged));
    assert_eq!(std::fs::read_dir(state.join("pending-deletes")).unwrap().count(), 0);
}

fn assert_visible_delete_stage(t: &TransferFixture, path: &FsPath) {
    assert_eq!(stat(path).unwrap().attrs & attr::HIDDEN, 0);
    let mut keep = |_: &ListingBuilder<'_>| true;
    let mut listing = ListingBuilder::new(CaseMode::Sensitive, &mut keep);
    NtFs.read_dir(&t.path("source"), &mut listing).unwrap();
    assert!((0..listing.listing.len() as u32).any(|i| {
        listing.listing.name(i) == path.file_name() && listing.listing.entries[i as usize].meta().attrs & attr::HIDDEN == 0
    }));
}

#[test]
fn copy_and_move_replacements_skip_shell_when_direct_recycling_is_unavailable() {
    for moving in [false, true] {
        let mut t = TransferFixture::new();
        t.shell_only = true;
        let source = t.write(r"source\data", b"new");
        let target = t.write(r"destination\data", b"old");
        let job = Job::detached(Resolve::Replace);
        let mut engine = Engine { recycle: &t, ..Engine::new(&job) };
        let err = engine.replace_staged(&source, &target, Mode::Copy, |staged| {
            std::fs::write(crate::fileops::std_path(staged), b"part of new").unwrap();
            assert_eq!(t.read(r"destination\data"), b"old");
            Err(ERROR_DISK_FULL.to_hresult().into())
        }).unwrap_err();
        assert_eq!(code(&err), ERROR_DISK_FULL);
        assert_eq!(t.read(r"destination\data"), b"old");
        assert_eq!(t.shell_attempts.get(), 0);
        drop(engine);

        let dest = target.parent().unwrap();
        let items = vec![source.clone()];
        let op = if moving { FileOp::Move { items, dest } } else { FileOp::Copy { items, dest } };
        let report = t.apply(op);
        assert_eq!(t.shell_attempts.get(), 0);
        assert_eq!(t.read(r"destination\data"), b"new");
        assert_eq!(exists(&source), !moving);
        assert_eq!(report.undo.replaced, vec![(target, None)]);
        assert_eq!(flit_core::undo::inverse(&report), Some(Step::Refuse(S::ErrorsOpsUndoBackup.en())));
    }
}

#[test]
fn declining_shell_recycling_cancels_copy_undo_and_keeps_the_copy() {
    let mut t = TransferFixture::new();
    let source = t.write(r"source\data", b"new");
    let target = t.path(r"destination\data");
    let report = t.apply(FileOp::Copy { items: vec![source], dest: target.parent().unwrap() });
    let step = flit_core::undo::inverse(&report).unwrap();
    t.shell_only = true;
    t.shell_refuses = true;
    let (undone, redo) = t.execute(step.clone());
    assert_eq!(undone.error.unwrap().message, S::ErrorsEngineDeletionCancelled.en());
    assert!(undone.items.is_empty() && redo.is_none());
    assert_eq!(t.shell_attempts.get(), 1);
    assert_eq!(t.read(r"destination\data"), b"new");

    t.shell_refuses = false;
    let (undone, redo) = t.execute(step);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert!(redo.is_some() && !exists(&target));
    assert_eq!(t.shell_attempts.get(), 2);
}

#[test]
fn consecutive_read_only_replacements_reuse_one_journal_record() {
    let mut t = TransferFixture::new();
    t.can_recycle = false;
    let state = t.dir.join("state");
    let records = state.join("pending-deletes");
    let job = Job::detached(Resolve::Replace);
    let mut engine = Engine { recycle: &t, pending_state: Some(state), ..Engine::new(&job) };
    let mut record = None;
    for name in ["one", "two"] {
        let from = t.write(&format!(r"source\{name}"), b"replacement");
        let to = t.write(&format!(r"destination\{name}"), b"read only");
        let mut permissions = std::fs::metadata(crate::fileops::std_path(&to)).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(crate::fileops::std_path(&to), permissions).unwrap();
        assert!(matches!(engine.transfer(&from, to, stat(&from).unwrap(), Mode::Copy, true), Res::Whole(_)));
        assert!(engine.error().is_none());
        let entries: Vec<_> = std::fs::read_dir(&records).unwrap().map(|entry| entry.unwrap().file_name()).collect();
        assert_eq!(entries.len(), 1);
        if let Some(first) = &record {
            assert_eq!(&entries[0], first);
        }
        record = entries.into_iter().next();
    }
    drop(engine);
    assert_eq!(std::fs::read_dir(&records).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(t.dir.join("destination")).unwrap().count(), 2);
    assert_eq!(t.read(r"destination\one"), b"replacement");
    assert_eq!(t.read(r"destination\two"), b"replacement");
}

#[test]
fn disk_full_after_a_real_copy_chunk_preserves_both_versions_and_cleans_the_stage() {
    let t = TransferFixture::new();
    let data = vec![0x5A; 16 << 20];
    let from = t.write(r"source\chunked", &data);
    let to = t.write(r"destination\chunked", b"old");
    let state = t.dir.join("state");
    let job = Job::detached(Resolve::Replace);
    let mut engine = Engine {
        recycle: &t,
        pending_state: Some(state.clone()),
        fail_after_chunk: Some(|done| (done > 0).then_some(ERROR_DISK_FULL)),
        ..Engine::new(&job)
    };
    let result = engine.transfer(&from, to, stat(&from).unwrap(), Mode::Across, true);
    assert!(matches!(result, Res::Nothing));
    assert!(engine.error().is_some());
    assert!((1..data.len() as u64).contains(&job.progress().bytes));
    assert_eq!(t.read(r"source\chunked"), data);
    assert_eq!(t.read(r"destination\chunked"), b"old");
    assert_eq!(std::fs::read_dir(t.dir.join("destination")).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(state.join("pending-deletes")).unwrap().count(), 0);
}

#[test]
fn failed_cross_volume_rollback_reports_the_recycled_backup_path() {
    let t = TransferFixture::new();
    let from = t.write(r"source\held", b"incoming");
    let to = t.write(r"destination\held", b"previous");
    use std::os::windows::fs::OpenOptionsExt;
    let source = std::fs::OpenOptions::new().read(true).share_mode(1).open(crate::fileops::std_path(&from)).unwrap();
    struct RefuseRestore<'a>(&'a TransferFixture);
    impl Recycle for RefuseRestore<'_> {
        fn direct(&self, path: &FsPath) -> Option<FsPath> { self.0.direct(path) }
        fn shell(&self, _: &FsPath, _: &Job) -> Result<FsPath, PlatformError> {
            panic!("native replacement must not call the Shell recycler")
        }
        fn restore(&self, _: &FsPath, _: &FsPath) -> bool { false }
    }
    let job = Job::detached(Resolve::Replace);
    let recycle = RefuseRestore(&t);
    let mut engine = Engine { recycle: &recycle, ..Engine::new(&job) };
    assert!(matches!(engine.transfer(&from, to, stat(&from).unwrap(), Mode::Across, true), Res::Nothing));
    drop(source);
    let records = job.take_replaced();
    let backup = records[0].1.as_ref().unwrap();
    assert!(engine.error().unwrap().message.contains(backup.display().as_ref()));
    assert_eq!(std::fs::read(crate::fileops::std_path(backup)).unwrap(), b"previous");
    assert_eq!(t.read(r"source\held"), b"incoming");
}

#[test]
fn reporting_a_large_directory_does_not_enumerate_descendants_and_trash_can_be_undone() {
    let mut t = TransferFixture::new();
    let original = t.path(r"source\large");
    std::fs::create_dir(crate::fileops::std_path(&original)).unwrap();
    let backup = t.put_in_bin(&original).unwrap();
    let fs = replay::FileSystem {
        stamp: undo_stamp,
        children: |_| panic!("reporting and restoring a recycled directory require only the selected root"),
    };
    let job = Job::detached(Resolve::Replace);
    let report = replay::report(FileOp::Trash(vec![original.clone()]), vec![(original.clone(), backup)], Some(fs), &job);
    let Some(Step::Checked(plan)) = flit_core::undo::inverse(&report) else { panic!("expected checked restore") };
    assert_eq!(plan.checks.len(), 2);
    let job = Job::new(flit_core::provider::OpCtl::checked(plan.clone()), Box::new(|_| {}), Box::new(|_| {}));
    assert!(replay::run(&mut t, &plan, Some(fs), &job).0.error.is_none());
    assert!(exists(&original));
}

#[test]
fn undo_checks_for_nested_changes_even_when_the_root_stamp_still_matches() {
    let mut t = TransferFixture::new();
    std::fs::create_dir(t.dir.join(r"source\tree")).unwrap();
    t.write(r"source\tree\child", b"original");
    let report = t.apply(FileOp::Copy { items: vec![t.path(r"source\tree")], dest: t.path("destination") });
    let child = t.dir.join(r"destination\tree\child");
    std::fs::File::options().write(true).open(child).unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(1)).unwrap();
    let (out, _) = t.execute(flit_core::undo::inverse(&report).unwrap());
    assert!(out.error.unwrap().message.contains(r"destination\tree\child"));
    assert!(out.items.is_empty());
    assert_eq!(t.read(r"destination\tree\child"), b"original");
}

#[test]
fn ordinary_copy_undo_keeps_a_backdated_addition_when_the_parent_stamp_changed() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, SystemTime};

    let mut t = TransferFixture::new();
    std::fs::create_dir(t.dir.join(r"source\tree")).unwrap();
    t.write(r"source\tree\child", b"original");
    let report = t.apply(FileOp::Copy { items: vec![t.path(r"source\tree")], dest: t.path("destination") });
    let undo = flit_core::undo::inverse(&report).unwrap();
    let root = t.dir.join(r"destination\tree");
    let directory = std::fs::OpenOptions::new().access_mode(FILE_WRITE_ATTRIBUTES.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0).open(&root).unwrap();
    let before = std::fs::metadata(&root).unwrap().modified().unwrap();
    let addition = t.write(r"destination\tree\added", b"backdated");
    std::fs::File::options().write(true).open(crate::fileops::std_path(&addition)).unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000)).unwrap();
    directory.set_modified(before + Duration::from_secs(1)).unwrap();
    let (refused, _) = t.execute(undo.clone());
    assert!(refused.error.is_some() && refused.items.is_empty());
    assert_eq!(t.read(r"destination\tree\added"), b"backdated");

    std::fs::remove_file(crate::fileops::std_path(&addition)).unwrap();
    directory.set_modified(before).unwrap();
    drop(directory);
    assert!(t.execute(undo).0.error.is_none());
    assert!(!root.exists());
    assert_eq!(t.read(r"source\tree\child"), b"original");
}

#[test]
fn undo_reports_when_the_file_identity_cannot_be_read() {
    let t = TransferFixture::new();
    let from = t.path(r"source\file");
    let to = t.path(r"destination\file");
    let fs = replay::FileSystem {
        stamp: |_| Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
        children: |_| unreachable!(),
    };
    let job = Job::detached(Resolve::Replace);
    let op = FileOp::MoveTo(vec![(from.clone(), to.clone())]);
    let report = replay::report(op, vec![(from, to)], Some(fs), &job);
    assert_eq!(flit_core::undo::inverse(&report), Some(Step::Refuse(S::ErrorsOpsUndoStamp.en())));
}


fn workspace_snapshot(path: &std::path::Path) -> std::collections::BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &std::path::Path, dir: &std::path::Path, out: &mut std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == "$Recycle.Bin" {
                continue;
            }
            let path = entry.path();
            let name = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                out.insert(name, None);
                visit(root, &path, out);
            } else {
                out.insert(name, Some(std::fs::read(path).unwrap()));
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    visit(path, path, &mut out);
    out
}

fn replay_property_config() -> proptest::test_runner::Config {
    use proptest::test_runner::{Config, FileFailurePersistence};
    let mut config = Config::default();
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 12;
    }
    config.failure_persistence = Some(Box::new(FileFailurePersistence::Direct(concat!(env!("CARGO_MANIFEST_DIR"), "/proptest-regressions/replay.txt"))));
    config
}

proptest::proptest! {
    #![proptest_config(replay_property_config())]
    #[test]
    fn filesystem_sequences_round_trip_through_undo_and_redo(
        operations in proptest::collection::vec((0u8..6, 0usize..5, proptest::collection::vec(proptest::num::u8::ANY, 0..96), proptest::collection::vec(proptest::num::u8::ANY, 0..96)), 1..8)
    ) {
        use proptest::prelude::{prop_assert, prop_assert_eq};
        let mut t = TransferFixture::new();
        let names = ["日本語.txt".to_owned(), "emoji-🧪.txt".into(), "single'quote".into(), "%20 encoded".into(), "long-".to_owned() + &"x".repeat(112)];
        let source = r"source\nested\deep";
        let target = r"destination\nested\deep";
        std::fs::create_dir_all(t.dir.join(source)).unwrap();
        std::fs::create_dir_all(t.dir.join(target)).unwrap();
        for (index, (mode, name, content, old)) in operations.iter().enumerate() {
            let name = &names[*name];
            t.write(&format!(r"{source}\{name}"), content);
            if index == 0 || *mode >= 4 {
                t.write(&format!(r"{target}\{name}"), old);
            }
        }
        let before = workspace_snapshot(&t.dir);
        let mut undos = Vec::new();
        let mut previous = None;
        for (index, (mode, choice, _, _)) in operations.into_iter().enumerate() {
            let mut paths: Vec<_> = [source, target].iter().flat_map(|folder| {
                std::fs::read_dir(t.dir.join(folder)).unwrap().map(|entry| {
                    FsPath::parse(entry.unwrap().path().to_str().unwrap()).unwrap()
                })
            }).collect();
            paths.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            let from = previous.take().filter(|path| paths.contains(path))
                .unwrap_or_else(|| paths[choice % paths.len()].clone());
            let dest = if from.parent().as_ref() == Some(&t.path(source)) { t.path(target) } else { t.path(source) };
            let op = match mode {
                0 | 4 => FileOp::Copy { items: vec![from], dest },
                1 | 5 => FileOp::Move { items: vec![from], dest },
                2 => FileOp::Rename { item: from, name: format!("renamed-{index}").into_bytes() },
                _ if paths.len() <= 1 => FileOp::Rename { item: from, name: format!("renamed-{index}").into_bytes() },
                _ => FileOp::Trash(vec![from]),
            };
            let report = t.apply(op);
            undos.push(flit_core::undo::inverse(&report).unwrap());
            if !matches!(report.op, FileOp::Trash(_)) {
                previous = report.items.first().map(|(_, to)| to.clone());
            }
        }
        let after = workspace_snapshot(&t.dir);
        let mut redos = Vec::new();
        for step in undos.into_iter().rev() {
            let (out, redo) = t.execute(step);
            prop_assert!(out.error.is_none(), "{:?}", out.error);
            redos.push(redo.unwrap());
        }
        prop_assert_eq!(workspace_snapshot(&t.dir), before);
        for step in redos.into_iter().rev() {
            let (out, _) = t.execute(step);
            prop_assert!(out.error.is_none(), "{:?}", out.error);
        }
        prop_assert_eq!(workspace_snapshot(&t.dir), after);
    }
}
