use super::*;
use std::cell::{Cell, RefCell};
use std::os::windows::ffi::OsStringExt;

#[test]
fn equal_unc_share_names_do_not_prove_that_an_unknown_volume_supports_hard_links() {
    let source = FsPath::parse(r"\\server\share\source.txt").unwrap();
    let destination = FsPath::parse(r"\\server\share\out").unwrap();
    assert_eq!(source.root_bytes(), destination.root_bytes());
    let root: Vec<u16> = r"\\server\share\".encode_utf16().chain([0]).collect();
    let unknown = identify_volume(&root, false, |read, _| {
        assert_eq!(read, root);
        Err(error(ERROR_NOT_SUPPORTED))
    });
    assert!(unknown.is_none());
    assert!(!same_volume(&source, &destination, unknown.as_ref(), unknown.as_ref()));
    assert_eq!(item_choices(&source, &destination, true, false, unknown.as_ref(), unknown.as_ref()).0, [true, true, false, false]);
    assert_ne!(item_choices(&source, &destination, true, false, None, None), LinkChoices::default());
    assert_eq!(item_choices(&source, &destination, false, true, None, None).0, [true, true, false, false]);
    let known = (r"\\?\Volume{known}\".encode_utf16().collect::<Vec<_>>(), true);
    let other = (r"\\?\Volume{other}\".encode_utf16().collect::<Vec<_>>(), true);
    assert!(same_volume(&source, &destination, Some(&known), Some(&known)));
    assert!(!same_volume(&source, &destination, Some(&known), Some(&other)));
    assert!(!same_volume(&source, &destination, Some(&known), None));
    assert_eq!(item_choices(&source, &destination, true, false, Some(&known), Some(&known)).0, [true, true, false, true]);
    assert_eq!(item_choices(&source, &destination, true, false, Some(&known), Some(&other)).0, [true, true, false, false]);
}

struct Temp(std::path::PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let dir = crate::test_temp().join(format!("flit-links-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, rel: &str) -> FsPath {
        FsPath::parse(self.0.join(rel).to_str().unwrap()).unwrap()
    }

    fn dir(&self, rel: &str) -> FsPath {
        std::fs::create_dir_all(self.0.join(rel)).unwrap();
        self.path(rel)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn elevated_arguments_preserve_unpaired_utf16_and_never_substitute_a_neighbor() {
    let t = Temp::new("utf16");
    let mut units: Vec<u16> = r"C:\source-".encode_utf16().collect();
    units.extend([0xD800, b'.' as u16, b't' as u16, b'x' as u16, b't' as u16]);
    let source = FsPath::from_utf16(&units).unwrap();
    let neighbor = FsPath::parse("C:\\source-\u{FFFD}.txt").unwrap();
    assert_ne!(encoded(&source), encoded(&neighbor));
    assert_eq!(decoded(OsString::from(encoded(&source))), Some(source.clone()));
    let path = t.dir("out").join(b"link with spaces");
    let launched = Cell::new(0);
    elevated_with(&[(source.clone(), path.clone())], |args| {
        launched.set(launched.get() + 1);
        assert_eq!(args.len(), 4);
        assert_eq!(args[0], "--create-symbolic-link");
        assert_eq!(decoded_id(&OsString::from(&args[1])), Some(folder_id(&t.path("out")).unwrap()));
        assert_eq!(decoded(OsString::from(&args[2])), Some(source.clone()));
        assert_eq!(decoded(OsString::from(&args[3])), Some(path.clone()));
        assert_eq!(OsString::from_wide(&units).encode_wide().collect::<Vec<_>>(), units);
        Ok(0)
    })
    .unwrap();
    assert_eq!(launched.get(), 1);
}

#[test]
fn a_whole_batch_of_symbolic_links_goes_through_one_elevated_launch() {
    let t = Temp::new("batch");
    let out = t.dir("out");
    let pairs: Vec<(FsPath, FsPath)> = (0..5).map(|k| (t.path(&format!("source {k}.txt")), out.join(format!("link {k}").as_bytes()))).collect();
    let launches = RefCell::new(Vec::new());
    elevated_with(&pairs, |args| {
        launches.borrow_mut().push(args.to_vec());
        Ok(0)
    })
    .unwrap();
    let launches = launches.into_inner();
    assert_eq!(launches.len(), 1);
    let args = &launches[0];
    assert_eq!(args.len(), 1 + 3 * pairs.len());
    for (k, (target, path)) in pairs.iter().enumerate() {
        assert_eq!(decoded(OsString::from(&args[2 + 3 * k])).as_ref(), Some(target));
        assert_eq!(decoded(OsString::from(&args[3 + 3 * k])).as_ref(), Some(path));
    }
    let failed = elevated_with(&pairs, |_| Ok(ERROR_ACCESS_DENIED.0));
    assert_eq!(WIN32_ERROR::from_error(&failed.unwrap_err()), Some(ERROR_ACCESS_DENIED));
    let refused = elevated_with(&pairs, |_| Err(flit_platform::PlatformError::new(ERROR_CANCELLED.0 as i64, "cancelled")));
    assert_eq!(WIN32_ERROR::from_error(&refused.unwrap_err()), Some(ERROR_CANCELLED));
}

#[test]
fn a_batch_longer_than_one_command_line_is_split_but_keeps_every_link() {
    let t = Temp::new("split");
    let out = t.dir("out");
    let long = "x".repeat(200);
    let pairs: Vec<(FsPath, FsPath)> = (0..40).map(|k| (t.path(&format!("{long}{k}")), out.join(format!("{long}{k}").as_bytes()))).collect();
    let launches = RefCell::new(Vec::new());
    elevated_with(&pairs, |args| {
        assert!(args.iter().map(|a| a.len() + 3).sum::<usize>() < 32767);
        launches.borrow_mut().push(args.len());
        Ok(0)
    })
    .unwrap();
    let launches = launches.into_inner();
    assert!(launches.len() > 1);
    assert_eq!(launches.iter().map(|n| n - 1).sum::<usize>(), 3 * pairs.len());
}

#[test]
fn malformed_ids_hex_nuls_extra_arguments_and_overlong_elevation_are_rejected() {
    let t = Temp::new("malformed");
    let out = t.dir("out");
    let path = out.join(b"link");
    let id = folder_id(&out).unwrap();
    for arg in ["", "0", "zzzz", "0043003a005c0000", "002f0074006d0070"] {
        assert!(decoded(OsString::from(arg)).is_none());
        assert_eq!(symbolic_from_command_line([OsString::from(&id), OsString::from(arg), OsString::from(encoded(&path))]), ERROR_INVALID_PARAMETER.0 as i32);
    }
    for bad in ["", "12", "zzzzzzzzzzzzzzzzzzzzzzzz", "0000000000000000000000000"] {
        assert_eq!(symbolic_from_command_line([OsString::from(bad), OsString::from(encoded(&path)), OsString::from(encoded(&path))]), ERROR_INVALID_PARAMETER.0 as i32);
    }
    assert_eq!(symbolic_from_command_line([OsString::from(encoded(&path)), OsString::from(encoded(&path))]), ERROR_INVALID_PARAMETER.0 as i32);
    assert_eq!(symbolic_from_command_line([OsString::from(&id), OsString::from(encoded(&path)), OsString::from(encoded(&path)), OsString::from("extra")]), ERROR_INVALID_PARAMETER.0 as i32);
    assert_eq!(symbolic_from_command_line(Vec::<OsString>::new()), ERROR_INVALID_PARAMETER.0 as i32);
    assert_eq!(symbolic_from_command_line([OsString::from(&id), OsString::from("relative"), OsString::from("also relative")]), ERROR_INVALID_PARAMETER.0 as i32);
    let long = FsPath::parse(&format!("C:\\{}", "a".repeat(9000))).unwrap();
    let result = elevated_with(&[(long, path.clone())], |_| panic!("overlong launch must be rejected before UAC"));
    assert_eq!(WIN32_ERROR::from_error(&result.unwrap_err()), Some(ERROR_FILENAME_EXCED_RANGE));
    assert_eq!(choices(&[], &FsPath::computer()), LinkChoices::default());
    assert!(!std_path(&path).exists());
}

#[test]
fn the_elevated_helper_refuses_a_staging_folder_that_was_swapped_for_a_junction_or_another_folder() {
    let t = Temp::new("swap");
    let target = t.dir("target");
    let real = t.dir("real");
    let other = t.dir("other");
    let via = t.path("via");
    junction(&real, &via).unwrap();
    let real_id = folder_id(&real).unwrap();
    assert!(folder_id(&via).is_err());
    let through = symbolic_from_command_line([OsString::from(&real_id), OsString::from(encoded(&target)), OsString::from(encoded(&via.join(b"link")))]);
    assert_eq!(through, ERROR_ACCESS_DENIED.0 as i32);
    let swapped = symbolic_from_command_line([OsString::from(&real_id), OsString::from(encoded(&target)), OsString::from(encoded(&other.join(b"link")))]);
    assert_eq!(swapped, ERROR_ACCESS_DENIED.0 as i32);
    assert_eq!(std::fs::read_dir(std_path(&real)).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(std_path(&other)).unwrap().count(), 0);
    let direct = symbolic_from_command_line([OsString::from(&real_id), OsString::from(encoded(&target)), OsString::from(encoded(&real.join(b"link")))]);
    assert!(direct == 0 || direct == ERROR_PRIVILEGE_NOT_HELD.0 as i32, "{direct}");
    std::fs::remove_dir(std_path(&via)).unwrap();
}

#[test]
fn symbolic_links_store_the_target_as_typed_without_the_long_path_prefix() {
    let t = Temp::new("plain");
    std::fs::write(t.0.join("target.txt"), "content").unwrap();
    let target = t.path("target.txt");
    let stored = String::from_utf16(&plain_target(&target)).unwrap();
    assert_eq!(stored, t.0.join("target.txt").to_str().unwrap());
    assert!(!stored.starts_with(r"\\?\"));
    let link = t.path("link.txt");
    match create(LinkKind::Symbolic, &target, &link) {
        Ok(()) => assert_eq!(std::fs::read_link(std_path(&link)).unwrap(), t.0.join("target.txt")),
        Err(e) => assert_eq!(WIN32_ERROR::from_error(&e), Some(ERROR_PRIVILEGE_NOT_HELD)),
    }
}

#[test]
fn junctions_point_at_the_path_as_given_without_resolving_links_inside_it() {
    let t = Temp::new("junction-path");
    t.dir("real\\inner");
    junction(&t.path("real"), &t.path("via")).unwrap();
    let target = t.path("via\\inner");
    let link = t.path("link");
    create(LinkKind::Junction, &target, &link).unwrap();
    assert_eq!(std::fs::read_link(std_path(&link)).unwrap(), t.0.join("via").join("inner"));
    assert!(t.0.join("link").is_dir());
    std::fs::remove_dir(t.0.join("link")).unwrap();
    std::fs::remove_dir(t.0.join("via")).unwrap();
}

#[test]
fn a_shortcut_to_a_broken_link_is_still_created() {
    let t = Temp::new("broken");
    let gone = t.dir("gone");
    let broken = t.path("broken");
    junction(&gone, &broken).unwrap();
    std::fs::remove_dir(std_path(&gone)).unwrap();
    assert!(std::fs::metadata(std_path(&broken)).is_err());
    let shortcut = t.path("broken.lnk");
    crate::com_sta();
    create(LinkKind::Shortcut, &broken, &shortcut).unwrap();
    assert!(std::fs::metadata(std_path(&shortcut)).unwrap().len() > 0);
    assert!(create(LinkKind::Junction, &broken, &t.path("again")).is_err());
    std::fs::remove_dir(std_path(&broken)).unwrap();
}
