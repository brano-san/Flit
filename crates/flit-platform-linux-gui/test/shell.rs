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
fn quoted_menu_arguments_come_back_whole_through_split_args() {
    let args: Vec<String> = ["/home/u/a b.txt", "it's", r#"x"'y"#, "", "''", "tab\there", "$(rm -rf ~) `x` ; |"].iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(split_args(&flit_core::cmdline::join(&args, true)), args);
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
fn kde_color_schemes_are_dark_when_the_window_background_is() {
    let breeze_dark = "[General]\nColorScheme=BreezeDark\n\n[Colors:View]\nBackgroundNormal=250,250,250\n\n[Colors:Window]\nBackgroundAlternate=49,54,59\nBackgroundNormal=32,35,38\n";
    assert_eq!(kde_dark(breeze_dark), Some(true));
    assert_eq!(kde_dark("[Colors:Window]\nBackgroundNormal=239,240,241\n"), Some(false));
    assert_eq!(kde_dark("[Colors:View]\nBackgroundNormal=20,20,20\n"), None);
    assert_eq!(kde_dark("[Colors:Window]\nBackgroundNormal=broken\n"), None);
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
    assert!(start.elapsed() < Duration::from_millis(200));
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

fn strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter().map(|a| a.to_string_lossy().into_owned()).collect()
}

#[test]
fn terminals_get_the_folder_through_their_own_option() {
    let dir = Path::new("/srv/my files");
    let args = |t: &str| strings(terminal_args(Path::new(t), Some(dir), None));
    assert_eq!(args("/usr/bin/gnome-terminal"), ["--working-directory=/srv/my files"]);
    assert_eq!(args("/usr/bin/gnome-terminal.wrapper"), ["--working-directory=/srv/my files"]);
    assert_eq!(args("ptyxis"), ["--working-directory=/srv/my files"]);
    assert_eq!(args("konsole"), ["--workdir", "/srv/my files"]);
    assert_eq!(args("alacritty"), ["--working-directory", "/srv/my files"]);
    assert_eq!(args("kitty"), ["--directory=/srv/my files"]);
    assert_eq!(args("wezterm"), ["start", "--cwd", "/srv/my files"]);
    assert!(args("xterm").is_empty(), "xterm starts in the working directory it inherits");
    assert_eq!(args("x-terminal-emulator"), ["-e", "sh", "-c", OPEN_SCRIPT, "flit", "/srv/my files"]);
    assert_eq!(args("xdg-terminal-exec"), ["sh", "-c", OPEN_SCRIPT, "flit", "/srv/my files"]);
}

#[test]
fn programs_run_in_terminals_with_each_terminal_s_syntax() {
    let cmd = || Some(vec![OsString::from("my app"), OsString::from("it's")]);
    let args = |t: &str| strings(terminal_args(Path::new(t), None, cmd()));
    assert_eq!(args("gnome-terminal"), ["--", "my app", "it's"]);
    assert_eq!(args("xfce4-terminal"), ["-x", "my app", "it's"]);
    assert_eq!(args("konsole"), ["-e", "my app", "it's"]);
    assert_eq!(args("foot"), ["my app", "it's"]);
    assert_eq!(args("wezterm"), ["start", "--", "my app", "it's"]);
    assert_eq!(args("tilix"), ["-e", r"'my app' 'it'\''s'"]);
    assert_eq!(args("xterm"), ["-e", "my app", "it's"]);
}

#[test]
fn the_run_script_starts_the_program_in_its_folder_and_keeps_a_shell() {
    let t = Temp::new("run-script");
    let out = t.path().join("out");
    let prog = t.path().join("prog dir");
    std::fs::create_dir(&prog).unwrap();
    let exe = prog.join("tool");
    std::fs::write(&exe, format!("#!/bin/sh\npwd > '{}'\n", out.display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let status = Command::new("sh").args(["-c", RUN_SCRIPT, "flit"]).arg(&prog).arg(&exe).env("SHELL", "/bin/true").status().unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read_to_string(&out).unwrap().trim_end(), prog.to_str().unwrap());
}

#[test]
fn only_executable_elf_programs_and_appimages_run_directly() {
    let t = Temp::new("direct");
    let sh = std::fs::read("/bin/sh").unwrap();
    assert!(elf_program(&sh[..sh.len().min(4096)]), "a dynamically linked program has an interpreter");
    let lib = ["/lib/x86_64-linux-gnu/libm.so.6", "/usr/lib/x86_64-linux-gnu/libm.so.6", "/usr/lib64/libm.so.6", "/usr/lib/libm.so.6"].into_iter().find_map(|p| std::fs::read(p).ok());
    if let Some(lib) = lib {
        assert!(!elf_program(&lib[..lib.len().min(4096)]), "a shared library is not a program");
    }
    assert!(!elf_program(b"#!/bin/sh\necho hi\n"));
    let copy = t.path().join("tool");
    std::fs::write(&copy, &sh).unwrap();
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!runs_directly(&copy), "a program without the execute bit opens like any file");
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(runs_directly(&copy));
    let script = t.path().join("script.sh");
    std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!runs_directly(&script), "scripts open in their editor instead of running");
    let app = t.path().join("Tool.AppImage");
    let mut static_header = sh[..64].to_vec();
    static_header[16] = 3;
    static_header[17] = 0;
    std::fs::write(&app, &static_header).unwrap();
    std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(runs_directly(&app), "an AppImage runs even when its runtime has no interpreter");
    assert!(!runs_directly(t.path()));
}

#[test]
fn a_program_without_the_execute_bit_asks_before_running() {
    let t = Temp::new("needs-exec");
    let app = t.path().join("Tool.AppImage");
    let mut elf = b"\x7fELF\x02\x01\x01".to_vec();
    elf.resize(64, 0);
    std::fs::write(&app, &elf).unwrap();
    std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(needs_exec_bit(&app));
    std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o744)).unwrap();
    assert!(!needs_exec_bit(&app));
    let text = t.path().join("notes.txt");
    std::fs::write(&text, b"hello").unwrap();
    assert!(!needs_exec_bit(&text));
}
