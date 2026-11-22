use super::*;
use flit_core::i18n::{self, Lang};
use windows::core::{Error, HRESULT};

fn language_test_child(test: &str) -> bool {
    const CHILD: &str = "FLIT_TEST_ENGINE_LANGUAGE";
    if std::env::var(CHILD).as_deref() == Ok(test) {
        return true;
    }
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .creation_flags(0x0800_0000)
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(CHILD, test)
        .output().unwrap();
    assert!(output.status.success(), "{test}:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    false
}

#[test]
fn staging_reservation_has_one_owner_and_never_removes_a_rivals_marker() {
    let root = crate::test_temp().join(format!("flit-w10-stage-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let reserved = FsPath::parse(root.join("stage").to_str().unwrap()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let actors: Vec<_> = (0..2).map(|_| {
        let (path, start) = (reserved.clone(), barrier.clone());
        std::thread::spawn(move || { start.wait(); LinkStage::reserve(path, LinkKind::Shortcut, false, None) })
    }).collect();
    let mut results: Vec<_> = actors.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results.iter().filter_map(|r| r.as_ref().err()).all(|e| code(e) == ERROR_ALREADY_EXISTS));
    let owner = results.remove(results.iter().position(Result::is_ok).unwrap()).unwrap();
    let marker = crate::fileops::std_path(&owner.path);
    std::fs::write(&marker, "rival marker").unwrap();
    assert!(LinkStage::reserve(reserved.clone(), LinkKind::Shortcut, false, None).is_err());
    drop(owner);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "rival marker");
    std::fs::remove_file(marker).unwrap();
    std::fs::remove_dir(crate::fileops::std_path(&reserved)).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn staging_cleanup_never_deletes_a_foreign_file_that_arrives_after_placement() {
    let root = crate::test_temp().join(format!("flit-w10-handoff-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = |name: &str| FsPath::parse(root.join(name).to_str().unwrap()).unwrap();
    let mut stage = LinkStage::reserve(path("stage"), LinkKind::Hard, false, None).unwrap();
    std::fs::write(crate::fileops::std_path(&stage.path), "owned link").unwrap();
    stage.made = true;
    let old_path = stage.path.clone();
    stage.place(&path("placed.txt"), false).unwrap();
    std::fs::write(crate::fileops::std_path(&old_path), "foreign marker").unwrap();
    drop(stage);
    let marker = std::fs::read_to_string(crate::fileops::std_path(&old_path));
    let placed = std::fs::read_to_string(root.join("placed.txt"));
    let _ = std::fs::remove_file(crate::fileops::std_path(&old_path));
    let _ = std::fs::remove_dir(root.join("stage"));
    std::fs::remove_file(root.join("placed.txt")).unwrap();
    std::fs::remove_dir(root).unwrap();
    assert_eq!(marker.unwrap(), "foreign marker");
    assert_eq!(placed.unwrap(), "owned link");
}

#[test]
fn windows_operation_and_shell_errors_follow_the_interface_language() {
    if !language_test_child("engine::tests::windows_operation_and_shell_errors_follow_the_interface_language") {
        return;
    }
    let original = i18n::lang();
    i18n::set_lang(Lang::Ru);
    let path = FsPath::parse(r"C:\missing.txt").unwrap();
    let error = Error::new(HRESULT(0x80070005u32 as i32), "system language");
    let expected = S::ErrorsOsPermissionDenied.t();
    let why = reason(&path, &error);
    assert_eq!(i18n::localize(&why), expected);
    let shell = crate::err(error);
    assert_eq!(shell.code, 0x80070005u32 as i32 as i64);
    assert_eq!(i18n::localize(&shell.message), expected);
    assert_eq!(i18n::localize(&shell.to_string()), format!("{expected} (0x80070005)"));
    let summary = tr_en!(S::ErrorsEngineFailed, count = 2, error = format!("{}: {why}", path.display()));
    assert_eq!(i18n::localize(&summary), flit_core::tr!(S::ErrorsEngineFailed, count = 2, error = format!("{}: {expected}", path.display())));
    i18n::set_lang(original);
}

#[test]
fn windows_errors_translate_known_codes_without_overwriting_unknown_system_reasons() {
    if !language_test_child("engine::tests::windows_errors_translate_known_codes_without_overwriting_unknown_system_reasons") {
        return;
    }
    let original = i18n::lang();
    i18n::set_lang(Lang::De);
    let path = FsPath::parse(r"C:\missing.txt").unwrap();
    let known = Error::new(HRESULT(0x80070003u32 as i32), "system language");
    assert_eq!(i18n::localize(&reason(&path, &known)), S::ErrorsOsNotFound.t());
    for code in [0x80004005u32, 0x80040005, 0x80071234] {
        let unknown = Error::new(HRESULT(code as i32), "a specific system reason");
        let expected = unknown.message();
        assert_eq!(reason(&path, &unknown), expected.trim_end());
        assert_eq!(crate::err(unknown).message, expected);
    }
    i18n::set_lang(original);
}

struct LinkTemp(std::path::PathBuf);

impl LinkTemp {
    fn new(tag: &str) -> Self {
        let dir = crate::test_temp().join(format!("flit-w10-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        Self(dir)
    }

    fn path(&self, rel: &str) -> FsPath {
        FsPath::parse(self.0.join(rel).to_str().unwrap()).unwrap()
    }

    fn file(&self, rel: &str) -> FsPath {
        std::fs::write(self.0.join(rel), rel).unwrap();
        self.path(rel)
    }

    fn out(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.0.join("out")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }
}

impl Drop for LinkTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn retrying_job(asked: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Job {
    let ctl = flit_core::provider::OpCtl::new(Some(Resolve::KeepBoth));
    let answer = ctl.clone();
    Job::asking(ctl, Box::new(|_| {}), Box::new(|_| {}), Box::new(move |failure: Failure| {
        assert_eq!(failure.error.kind, PlatformErrorKind::SymlinkPrivilege);
        assert_eq!(failure.error.message, S::ErrorsEngineSymlinkPrivilege.en());
        asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        answer.answer_failure(Recover::Retry, false);
    }))
}

fn unprivileged(kind: LinkKind, _: &FsPath, _: &FsPath) -> windows::core::Result<()> {
    assert_eq!(kind, LinkKind::Symbolic);
    Err(ERROR_PRIVILEGE_NOT_HELD.to_hresult().into())
}

#[test]
fn one_retry_as_administrator_creates_every_remaining_symbolic_link_in_one_elevated_launch() {
    let t = LinkTemp::new("batch");
    let items: Vec<FsPath> = (0..4).map(|k| t.file(&format!("source {k}.txt"))).collect();
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let job = retrying_job(asked.clone());
    let launches = std::cell::RefCell::new(Vec::new());
    let elevate = |pairs: &[(FsPath, FsPath)]| {
        launches.borrow_mut().push(pairs.len());
        for (target, path) in pairs {
            std::fs::write(crate::fileops::std_path(path), crate::fileops::std_path(target).to_string_lossy().as_bytes()).unwrap();
        }
        Ok(())
    };
    let mut engine = Engine::new(&job);
    engine.make_link = &unprivileged;
    engine.elevate_links = &elevate;
    engine.links(&items, &t.path("out"), LinkKind::Symbolic);
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(*launches.borrow(), [4]);
    assert!(engine.error().is_none());
    assert_eq!(engine.done.len(), 4);
    assert_eq!(t.out(), ["Link to source 0.txt", "Link to source 1.txt", "Link to source 2.txt", "Link to source 3.txt"]);
}

#[test]
fn refusing_the_administrator_prompt_reports_every_link_as_failed_instead_of_cancelling_silently() {
    let t = LinkTemp::new("refused");
    let items: Vec<FsPath> = (0..2).map(|k| t.file(&format!("source {k}.txt"))).collect();
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let job = retrying_job(asked.clone());
    let refuse = |_: &[(FsPath, FsPath)]| Err(ERROR_CANCELLED.to_hresult().into());
    let mut engine = Engine::new(&job);
    engine.make_link = &unprivileged;
    engine.elevate_links = &refuse;
    engine.links(&items, &t.path("out"), LinkKind::Symbolic);
    assert!(!job.cancelled());
    let error = engine.error().unwrap();
    assert_eq!(error.code, ERROR_PRIVILEGE_NOT_HELD.0 as i64);
    assert!(error.message.contains(S::ErrorsEngineSymlinkPrivilege.en()));
    assert!(error.message.contains('2'));
    assert!(engine.done.is_empty());
    assert!(t.out().is_empty());
}

#[test]
fn declining_the_retry_asks_once_and_never_launches_an_elevated_process() {
    let t = LinkTemp::new("declined");
    let items: Vec<FsPath> = (0..3).map(|k| t.file(&format!("source {k}.txt"))).collect();
    for cancel in [false, true] {
        let ctl = flit_core::provider::OpCtl::new(Some(Resolve::KeepBoth));
        let answer = ctl.clone();
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = asked.clone();
        let job = Job::asking(ctl, Box::new(|_| {}), Box::new(|_| {}), Box::new(move |_| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if cancel { answer.cancel(); } else { answer.answer_failure(Recover::Skip, false); }
        }));
        let never = |_: &[(FsPath, FsPath)]| -> windows::core::Result<()> { panic!("elevated after declining") };
        let mut engine = Engine::new(&job);
        engine.make_link = &unprivileged;
        engine.elevate_links = &never;
        engine.links(&items, &t.path("out"), LinkKind::Symbolic);
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(job.cancelled(), cancel);
        let error = engine.error().unwrap();
        let kind = if cancel { flit_platform::PlatformErrorKind::Cancelled } else { flit_platform::PlatformErrorKind::AccessDenied };
        assert_eq!(error.kind, kind);
        assert_eq!(error.code, if cancel { ERROR_CANCELLED.0 as i64 } else { ERROR_PRIVILEGE_NOT_HELD.0 as i64 });
        assert!(t.out().is_empty());
    }
}

#[test]
fn the_reported_error_code_belongs_to_the_first_reported_message() {
    let t = LinkTemp::new("first-code");
    let items = vec![t.path("missing.txt"), t.file("present.txt")];
    let job = Job::detached(Resolve::KeepBoth);
    let mut engine = Engine::new(&job);
    engine.make_link = &unprivileged;
    engine.links(&items, &t.path("out"), LinkKind::Symbolic);
    let error = engine.error().unwrap();
    assert!(error.message.contains("missing.txt"));
    assert_eq!(error.code, -1);
    let job = Job::detached(Resolve::KeepBoth);
    let mut engine = Engine::new(&job);
    engine.make_link = &unprivileged;
    engine.links(&items[1..], &t.path("out"), LinkKind::Symbolic);
    assert_eq!(engine.error().unwrap().code, ERROR_PRIVILEGE_NOT_HELD.0 as i64);
}

#[test]
fn copy_move_and_delete_ignore_a_drive_root_while_links_can_still_point_at_it() {
    let root = FsPath::parse_bytes(FsPath::parse(crate::test_temp().to_str().unwrap()).unwrap().root_bytes()).unwrap();
    assert!(stat(&root).is_none());
    assert!(link_stat(&root).is_some_and(|m| is_dir(&m)));
    let job = Job::detached(Resolve::Skip);
    let mut engine = Engine::new(&job);
    assert!(engine.stat_all(std::slice::from_ref(&root)).iter().all(Option::is_none));
}

fn with_companions(label: &str, test: impl FnOnce(&FsPath, &FsPath, &FsPath, &FsPath)) {
    let root = crate::test_temp().join(format!("flit-companions-{label}-{}", std::process::id()));
    std::fs::create_dir_all(root.join("to")).unwrap();
    let dir = FsPath::parse(root.to_str().unwrap()).unwrap();
    let dest = dir.join(b"to");
    let owner = dir.join(b"photo.jpg");
    let sidecar = dir.join(b"photo.jpg.xmp");
    std::fs::write(flit_platform::std_path(&owner), b"photo").unwrap();
    std::fs::write(flit_platform::std_path(&sidecar), b"metadata").unwrap();
    test(&dir, &dest, &owner, &sidecar);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_duplicate_uses_the_main_files_copy_name_for_its_companion() {
    with_companions("duplicate", |dir, _, owner, sidecar| {
        let job = Job::detached(Resolve::Skip);
        let op = FileOp::Copy { items: vec![owner.clone(), sidecar.clone()], dest: dir.clone() };
        let op = job.prepare_companions(&NtFs, op).unwrap();
        let out = run(&op, &job);
        assert!(out.error.is_none(), "{:?}", out.error);
        assert_eq!(out.items, [
            (owner.clone(), dir.join(b"photo - Copy.jpg")),
            (sidecar.clone(), dir.join(b"photo - Copy.jpg.xmp")),
        ]);
        assert_eq!(std::fs::read(flit_platform::std_path(&dir.join(b"photo - Copy.jpg.xmp"))).unwrap(), b"metadata");
    });
}

#[test]
fn a_primary_conflict_can_skip_rename_or_replace_a_group_and_sidecar_collisions_stop_it() {
    for moving in [false, true] {
        let label = if moving { "move-conflict" } else { "copy-conflict" };
        with_companions(label, |_, dest, owner, sidecar| {
            let target = dest.join(owner.file_name());
            let side_target = dest.join(sidecar.file_name());
            std::fs::write(flit_platform::std_path(&target), b"existing photo").unwrap();
            std::fs::write(flit_platform::std_path(&side_target), b"existing sidecar").unwrap();
            let perform = |choice| {
                let ctl = flit_core::provider::OpCtl::new(None);
                let answer = ctl.clone();
                let wanted = target.clone();
                let job = Job::new(ctl, Box::new(|_| {}), Box::new(move |conflict| {
                    assert_eq!(conflict.target, wanted);
                    answer.answer(choice, false);
                }));
                let op = if moving {
                    FileOp::Move { items: vec![owner.clone()], dest: dest.clone() }
                } else {
                    FileOp::Copy { items: vec![owner.clone()], dest: dest.clone() }
                };
                run(&job.prepare_companions(&NtFs, op).unwrap(), &job)
            };
            let skipped = perform(Resolve::Skip);
            assert!(skipped.error.is_none() && skipped.items.is_empty());
            let refused = perform(Resolve::Replace);
            assert!(refused.error.unwrap().to_string().contains("photo.jpg.xmp"));
            assert!(refused.items.is_empty());
            let renamed = perform(Resolve::KeepBoth);
            assert!(renamed.error.is_none(), "{:?}", renamed.error);
            assert_eq!(renamed.items, [
                (owner.clone(), dest.join(b"photo (2).jpg")),
                (sidecar.clone(), dest.join(b"photo (2).jpg.xmp")),
            ]);
            assert_eq!(std::fs::read(flit_platform::std_path(&side_target)).unwrap(), b"existing sidecar");
            assert_eq!(std::fs::read(flit_platform::std_path(&target)).unwrap(), b"existing photo");
            assert_eq!(exists(owner), !moving);
            assert_eq!(exists(sidecar), !moving);
        });
    }
}

#[test]
fn cancellation_after_the_main_file_moves_finishes_its_companion_group() {
    with_companions("cancel", |_, dest, owner, sidecar| {
        let job = Job::detached(Resolve::Skip);
        let op = FileOp::Move { items: vec![owner.clone()], dest: dest.clone() };
        job.prepare_companions(&NtFs, op).unwrap();
        let target = dest.join(owner.file_name());
        let side_target = dest.join(sidecar.file_name());
        let mut engine = Engine::new(&job);
        let moved = engine.transfer(owner, target.clone(), stat(owner).unwrap(), Mode::Move, true);
        assert!(matches!(moved, Res::Whole(_)));
        job.ctl().cancel();
        engine.keep_group(owner, moved, Mode::Move);
        assert!(engine.stopped());
        assert_eq!(engine.done, [(owner.clone(), target), (sidecar.clone(), side_target.clone())]);
        assert!(!exists(sidecar));
        assert_eq!(std::fs::read(flit_platform::std_path(&side_target)).unwrap(), b"metadata");
    });
}
