use super::*;
use flit_core::i18n::{self, Lang};
use windows::core::{Error, HRESULT};

#[test]
fn staging_reservation_has_one_owner_and_never_removes_a_rivals_marker() {
    let root = crate::test_temp().join(format!("flit-w10-stage-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let reserved = FsPath::parse(root.join("stage").to_str().unwrap()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let actors: Vec<_> = (0..2).map(|_| {
        let (path, start) = (reserved.clone(), barrier.clone());
        std::thread::spawn(move || { start.wait(); LinkStage::reserve(path, LinkKind::Shortcut, false) })
    }).collect();
    let mut results: Vec<_> = actors.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results.iter().filter_map(|r| r.as_ref().err()).all(|e| code(e) == ERROR_ALREADY_EXISTS));
    let owner = results.remove(results.iter().position(Result::is_ok).unwrap()).unwrap();
    let marker = crate::fileops::std_path(&owner.path);
    std::fs::write(&marker, "rival marker").unwrap();
    assert!(LinkStage::reserve(reserved.clone(), LinkKind::Shortcut, false).is_err());
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
    let mut stage = LinkStage::reserve(path("stage"), LinkKind::Hard, false).unwrap();
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
