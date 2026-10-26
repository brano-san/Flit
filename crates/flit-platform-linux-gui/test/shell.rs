use super::*;
use crate::temp::Temp;
use flit_core::provider::{Resolve, Template};

fn fs_path(p: &Path) -> FsPath {
    FsPath::parse_exact(p.as_os_str().as_bytes()).unwrap()
}

#[test]
fn posters_come_from_pdftoppm_and_ffmpegthumbnailer() {
    let pdf = poster_command(Path::new("/d/a b.PDF"), 256).unwrap();
    assert_eq!(pdf.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>(), ["pdftoppm", "-png", "-f", "1", "-l", "1", "-singlefile", "-scale-to", "256", "/d/a b.PDF", "-"]);
    if std::path::Path::new("/usr/share/mime/globs2").exists() {
        let video = poster_command(Path::new("/d/clip.mp4"), 128).unwrap();
        assert_eq!(video[0], "ffmpegthumbnailer");
        assert_eq!(video.last().unwrap(), "/d/clip.mp4");
    }
    assert!(poster_command(Path::new("/d/notes.txt"), 128).is_none());
    let t = Temp::new("poster");
    let pdf = t.path().join("one.pdf");
    let made = std::process::Command::new("convert").args(["-size", "200x300", "xc:blue", pdf.to_str().unwrap()]).status().is_ok_and(|s| s.success());
    if made && on_path(OsStr::new("pdftoppm")).is_some() {
        let b = poster(&pdf, 120).unwrap();
        assert_eq!(b.height, 120);
        assert!(b.rgba[2] > 200);
    }
}

#[test]
fn file_lists_round_trip_through_uri_lists_and_gnome_cut_markers() {
    let paths = vec![FsPath::parse_exact(b"/tmp/a b/\xD0\xBC%.txt").unwrap(), FsPath::parse_exact(b"/x\xFF").unwrap()];
    let list = uri_list(&paths);
    assert_eq!(list, "file:///tmp/a%20b/%D0%BC%25.txt\r\nfile:///x%FF\r\n");
    assert_eq!(parse_file_list(list.as_bytes()), (paths.clone(), false));
    assert_eq!(parse_file_list(b"# comment\nfile://localhost/x%FF\nhttp://e/x\n"), (vec![paths[1].clone()], false));
    assert_eq!(parse_file_list(b"cut\nfile:///x%FF"), (vec![paths[1].clone()], true));
    assert_eq!(parse_file_list(b"copy\nfile:///x%FF\n"), (vec![paths[1].clone()], false));
    remember_files(&paths, true);
    let mut sh = LinuxShell;
    if clipboard_bytes("text/uri-list").is_none() {
        assert_eq!(sh.clipboard_files().unwrap(), (paths, true));
    }
    sh.clear_clipboard();
    if clipboard_bytes("text/uri-list").is_none() {
        assert!(sh.clipboard_files().is_err());
    }
}

#[test]
fn arguments_split_on_spaces_and_respect_quotes() {
    assert_eq!(split_args(r#"-a "two words" 'single quoted' plain"#), ["-a", "two words", "single quoted", "plain"]);
    assert_eq!(split_args(r#"  "" x  "#), ["", "x"]);
    assert!(split_args("   ").is_empty());
}

#[test]
fn color_scheme_replies_from_portal_and_gsettings() {
    assert_eq!(parse_color_scheme(b"(<<uint32 1>>,)\n"), Some(true));
    assert_eq!(parse_color_scheme(b"(<<uint32 2>>,)\n"), Some(false));
    assert_eq!(parse_color_scheme(b"(<<uint32 0>>,)\n"), None);
    assert_eq!(parse_color_scheme(b"'prefer-dark'\n"), Some(true));
    assert_eq!(parse_color_scheme(b"'default'\n"), Some(false));
    assert_eq!(parse_color_scheme(b"garbage"), None);
}

#[test]
fn programs_are_found_on_path_and_spawned_without_a_shell() {
    assert!(on_path(OsStr::new("sh")).is_some());
    assert!(on_path(OsStr::new("flit-no-such-program")).is_none());
    assert!(on_path(OsStr::new("/bin/sh")).is_some());
    let t = Temp::new("spawn");
    let marker = t.path().join("ran; touch injected");
    let mut cmd = Command::new("touch");
    cmd.arg(&marker);
    detach(cmd).unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    while !marker.exists() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(marker.exists());
    assert!(!t.path().join("injected").exists());
    assert!(launch(&Launch::Terminal { dirs: vec![fs_path(t.path())], admin: true }).is_err());
    assert!(launch(&Launch::Eject(fs_path(t.path()))).is_err());
}

#[test]
fn theme_probe_never_blocks_the_caller() {
    let start = Instant::now();
    let (tx, rx) = crossbeam_channel::bounded(1);
    dark_preference(move |dark| {
        let _ = tx.send(dark);
    });
    assert!(start.elapsed() < Duration::from_millis(20));
    let _ = rx.recv_timeout(Duration::from_secs(2));
    let _ = clipboard_text();
}

fn safe_trash_home() -> bool {
    crate::xdg::data_home().is_some_and(|d| d.as_os_str().as_bytes().starts_with(b"/tmp/flit-"))
}

#[test]
fn file_ops_create_from_templates_and_trash_one_file_quickly() {
    assert!(safe_trash_home(), "tests must run with XDG_DATA_HOME or HOME under /tmp/flit-*");
    let t = Temp::new("ops");
    let template = t.file("tpl/Note.md", 4);
    let mut sh = LinuxShell::new().unwrap();
    let job = Job::detached(Resolve::Skip);
    let dir = fs_path(t.path());
    let made = sh.file_op(&FileOp::NewFile { dir: dir.clone(), name: b"Note.md".to_vec(), template: Template::Copy(fs_path(&template)) }, &job);
    assert!(made.error.is_none(), "{:?}", made.error);
    let again = sh.file_op(&FileOp::NewFile { dir: dir.clone(), name: b"Note.md".to_vec(), template: Template::Data(b"hi".to_vec()) }, &job);
    assert_eq!(again.items[0].0, dir.join(b"Note (2).md"));
    assert_eq!(std::fs::read(t.path().join("Note.md")).unwrap().len(), 4);
    assert_eq!(std::fs::read(t.path().join("Note (2).md")).unwrap(), b"hi");
    let bad = sh.file_op(&FileOp::NewFile { dir: dir.clone(), name: b"a/b".to_vec(), template: Template::Empty }, &job);
    assert!(bad.error.is_some());
    let mut took = Vec::new();
    for k in 0..21 {
        let victim = t.file(&format!("victim-{k}"), 10);
        let start = Instant::now();
        let out = sh.file_op(&FileOp::Trash(vec![fs_path(&victim)]), &job);
        took.push(start.elapsed());
        assert!(out.error.is_none(), "{:?}", out.error);
        assert!(!victim.exists());
        let (from, to) = &out.items[0];
        assert_eq!(from, &fs_path(&victim));
        let undo = sh.file_op(&FileOp::Restore(vec![(to.clone(), from.clone())]), &job);
        assert!(undo.error.is_none(), "{:?}", undo.error);
        assert!(victim.exists());
    }
    took.sort();
    eprintln!("measure: trash of one file median {:?}, max {:?}", took[10], took[20]);
}
