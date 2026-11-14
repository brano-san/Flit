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
fn double_click_replies_from_portal_and_gsettings() {
    assert_eq!(last_number("(<<400>>,)\n"), Some(400));
    assert_eq!(last_number("(<<int32 250>>,)\n"), Some(250), "the type tag's digits are not the value");
    assert_eq!(last_number("650\n"), Some(650));
    assert_eq!(last_number(""), None);
    assert_eq!(last_number("No such key “double-click”\n"), None);
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
    assert!(launch(&Launch::Eject(fs_path(t.path()))).is_err());
}

#[test]
fn theme_probe_never_blocks_the_caller() {
    let start = Instant::now();
    let (tx, rx) = crossbeam_channel::bounded(1);
    dark_preference(
        move |dark| {
            let _ = tx.send(dark);
        },
        |_| {},
    );
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
    assert_eq!(args("kgx"), ["--working-directory=/srv/my files"]);
    assert_eq!(args("ptyxis"), ["--new-window", "--working-directory=/srv/my files"], "without --new-window Ptyxis raises an old window or opens home");
    assert_eq!(args("konsole"), ["--workdir", "/srv/my files"]);
    assert_eq!(args("qterminal"), ["--workdir", "/srv/my files"]);
    assert_eq!(args("alacritty"), ["--working-directory", "/srv/my files"]);
    assert_eq!(args("cosmic-term"), ["--working-directory", "/srv/my files"]);
    assert_eq!(args("kitty"), ["--directory=/srv/my files"]);
    assert_eq!(args("wezterm"), ["start", "--cwd", "/srv/my files"]);
    assert!(args("xterm").is_empty(), "xterm starts in the working directory it inherits");
    assert!(args("st").is_empty());
    assert_eq!(args("x-terminal-emulator"), ["-e", "sh", "-c", OPEN_SCRIPT, "flit", "/srv/my files"]);
    assert_eq!(args("/usr/bin/gnome-terminal.wrapper"), ["-e", "sh", "-c", OPEN_SCRIPT, "flit", "/srv/my files"], "a wrapper drops options xterm lacks");
    assert_eq!(args("xdg-terminal-exec"), ["sh", "-c", OPEN_SCRIPT, "flit", "/srv/my files"]);
}

fn executable_file(p: &Path) {
    std::fs::write(p, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn an_alternatives_chain_to_a_wrapper_opens_the_real_terminal_in_the_folder() {
    let t = Temp::new("terminal-chain");
    let bin = t.path().join("bin");
    let alternatives = t.path().join("etc/alternatives");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&alternatives).unwrap();
    executable_file(&bin.join("gnome-terminal.wrapper"));
    executable_file(&bin.join("gnome-terminal"));
    std::os::unix::fs::symlink(bin.join("gnome-terminal.wrapper"), alternatives.join("x-terminal-emulator")).unwrap();
    std::os::unix::fs::symlink(alternatives.join("x-terminal-emulator"), bin.join("x-terminal-emulator")).unwrap();
    let program = real_terminal(&bin.join("x-terminal-emulator"), |_| None);
    assert_eq!(program, std::fs::canonicalize(bin.join("gnome-terminal")).unwrap());
    assert_eq!(strings(terminal_args(&program, Some(Path::new("/srv/my files")), None)), ["--working-directory=/srv/my files"]);
    let run = strings(terminal_args(&program, Some(Path::new("/srv")), Some(vec!["sh".into(), "-c".into()])));
    assert_eq!(run, ["--working-directory=/srv", "--", "sh", "-c"]);
    std::fs::remove_file(bin.join("gnome-terminal")).unwrap();
    let elsewhere = t.path().join("usr/bin/gnome-terminal");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    executable_file(&elsewhere);
    let found = real_terminal(&bin.join("x-terminal-emulator"), |name| (name == "gnome-terminal").then(|| elsewhere.clone()));
    assert_eq!(found, elsewhere);
}

#[test]
fn unknown_or_unwrappable_terminals_keep_the_dispatcher_and_its_script() {
    let t = Temp::new("terminal-unknown");
    let wrapper = t.path().join("gnome-terminal.wrapper");
    executable_file(&wrapper);
    let x = t.path().join("x-terminal-emulator");
    std::os::unix::fs::symlink(&wrapper, &x).unwrap();
    assert_eq!(real_terminal(&x, |_| None), x, "a wrapper without its terminal stays the xterm-compatible entry");
    let odd = t.path().join("odd-term");
    executable_file(&odd);
    let y = t.path().join("y").join("x-terminal-emulator");
    std::fs::create_dir_all(y.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&odd, &y).unwrap();
    assert_eq!(real_terminal(&y, |_| None), y);
    assert_eq!(strings(terminal_args(&y, Some(Path::new("/srv")), None)), ["-e", "sh", "-c", OPEN_SCRIPT, "flit", "/srv"]);
    let missing = t.path().join("gone");
    assert_eq!(real_terminal(&missing, |_| None), missing);
    let konsole = t.path().join("konsole");
    executable_file(&konsole);
    assert_eq!(real_terminal(&konsole, |_| None), std::fs::canonicalize(&konsole).unwrap());
}

#[test]
fn programs_run_in_terminals_with_each_terminal_s_syntax() {
    let cmd = || Some(vec![OsString::from("my app"), OsString::from("it's")]);
    let args = |t: &str| strings(terminal_args(Path::new(t), None, cmd()));
    assert_eq!(args("gnome-terminal"), ["--", "my app", "it's"]);
    assert_eq!(args("ptyxis"), ["--new-window", "--", "my app", "it's"]);
    assert_eq!(args("x-terminal-emulator"), ["-e", "my app", "it's"]);
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

#[test]
fn copied_files_go_through_the_clipboard_helper_as_a_uri_list_that_dolphin_and_nautilus_both_paste() {
    use crate::clipboard::{files, URI_LIST};
    let paths = [FsPath::parse("/tmp/a b").unwrap(), FsPath::parse("/tmp/c").unwrap()];
    let (mime, bytes) = helper_offer(&paths, false);
    assert_eq!((mime, bytes.as_slice()), (URI_LIST, b"file:///tmp/a%20b\r\nfile:///tmp/c\r\n".as_slice()));
    assert_eq!(files(&[(mime.to_owned(), bytes)]), Some((paths.to_vec(), false)));
}

#[test]
fn cut_files_go_through_the_clipboard_helper_with_the_cut_mark_kept() {
    use crate::clipboard::{files, GNOME};
    let paths = [FsPath::parse("/tmp/a b").unwrap()];
    let (mime, bytes) = helper_offer(&paths, true);
    assert_eq!((mime, bytes.as_slice()), (GNOME, b"cut\nfile:///tmp/a%20b".as_slice()));
    assert_eq!(files(&[(mime.to_owned(), bytes)]), Some((paths.to_vec(), true)));
}

#[test]
fn launcher_progress_moves_in_steps_of_five_percent() {
    use crate::window::launcher_step;
    assert_eq!(launcher_step(None), None);
    assert_eq!(launcher_step(Some(0)), Some(0));
    assert_eq!(launcher_step(Some(4)), Some(0));
    assert_eq!(launcher_step(Some(37)), Some(35));
    assert_eq!(launcher_step(Some(100)), Some(100));
    assert_eq!(launcher_step(Some(250)), Some(100));
}

#[test]
fn root_terminals_keep_the_folder_through_sudo_or_pkexec() {
    let dir = Path::new("/srv/my site");
    assert_eq!(root_shell(dir, true), ["sudo", "-s"].map(OsString::from));
    let pk = root_shell(dir, false);
    assert_eq!(pk[0], "pkexec");
    assert_eq!(pk.last().unwrap(), "/srv/my site", "the folder is an argument, never part of the script");
    assert!(!pk[3].to_string_lossy().contains("my site"));
    let out = Command::new("/bin/sh").args(&pk[2..]).env("SHELL", "/bin/pwd").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "", "a missing folder stops the script before a shell starts");
    let t = Temp::new("root-shell");
    let here = root_shell(t.path(), false);
    let out = Command::new("/bin/sh").args(&here[2..]).env("SHELL", "/bin/pwd").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), t.path().to_str().unwrap());
}

fn write_mode(path: &Path, text: &str, mode: u32) -> PathBuf {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    path.to_path_buf()
}

fn text_editor() -> crate::apps::Entry {
    crate::apps::parse_desktop("editor.desktop", Path::new("/usr/share/applications/editor.desktop"), "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor %F\nMimeType=text/plain;\n", "").unwrap()
}

#[test]
fn an_executable_script_asks_before_running_and_a_file_without_an_app_offers_the_chooser() {
    let t = Temp::new("open-ask");
    let entries = [text_editor()];
    let assoc = crate::apps::Assoc::default();
    let ask = |p: &Path| open_question(p, &entries, &assoc);
    let script = write_mode(&t.path().join("build.sh"), "#!/bin/sh\necho hi\n", 0o755);
    assert_eq!(ask(&script), Some(OpenAsk::Script));
    let blob = write_mode(&t.path().join("blob.flit-unknown-type"), "\0\x01\x02binary", 0o644);
    assert!(matches!(ask(&blob), Some(OpenAsk::NoApp { mime, apps }) if mime == "application/octet-stream" && apps.len() == 1));
}

#[test]
fn ordinary_files_open_without_questions() {
    let t = Temp::new("open-plain");
    let entries = [text_editor()];
    let assoc = crate::apps::Assoc::default();
    let ask = |p: &Path| open_question(p, &entries, &assoc);
    assert_eq!(ask(&write_mode(&t.path().join("notes.txt"), "words\n", 0o644)), None);
    assert_eq!(ask(&write_mode(&t.path().join("on-fat.txt"), "every file on a FAT drive is executable\n", 0o755)), None, "no shebang, no question");
    assert_eq!(ask(&write_mode(&t.path().join("plain.sh"), "#!/bin/sh\necho hi\n", 0o644)), None, "a script without the execute bit just opens");
    assert_eq!(ask(&write_mode(&t.path().join("app.desktop"), "#!x\n[Desktop Entry]\n", 0o755)), None, "launchers have their own trust check");
    assert_eq!(ask(t.path()), None);
    let blob = write_mode(&t.path().join("blob.flit-unknown-type"), "\0\x01\x02binary", 0o644);
    assert_eq!(open_question(&blob, &[], &assoc), None, "without any applications xdg-open gets its chance");
}

#[test]
fn launchers_need_trust_unless_installed_or_executable_and_links_lead_to_folders() {
    let t = Temp::new("launchers");
    let app = "[Desktop Entry]\nType=Application\nName=Game\nIcon=game-icon\nExec=game\n";
    let untrusted = write_mode(&t.path().join("Game.desktop"), app, 0o644);
    assert!(needs_exec_bit(&untrusted));
    assert!(!needs_exec_bit(&write_mode(&t.path().join("Mine.desktop"), app, 0o744)));
    let apps = t.path().join("share/applications/kde");
    std::fs::create_dir_all(&apps).unwrap();
    let installed = write_mode(&apps.join("game.desktop"), app, 0o644);
    assert!(crate::apps::trusted_launcher(&installed, &[t.path().join("share")]));
    assert!(!crate::apps::trusted_launcher(&untrusted, &[t.path().join("share")]));
    assert_eq!(crate::apps::launcher_icon(app).as_deref(), Some("game-icon"));
    let docs = t.path().join("Docs dir");
    std::fs::create_dir_all(&docs).unwrap();
    let link = |name: &str, url: &str| write_mode(&t.path().join(name), &format!("[Desktop Entry]\nType=Link\nName=L\nURL={url}\n"), 0o644);
    assert_eq!(launcher_folder(&link("docs.desktop", &format!("file://{}", docs.display()).replace(' ', "%20"))), Some(docs.clone()));
    assert_eq!(launcher_folder(&link("plain.desktop", &docs.display().to_string())), Some(docs.clone()));
    assert_eq!(launcher_folder(&link("web.desktop", "https://example.org/")), None);
    assert_eq!(launcher_folder(&untrusted), None);
    assert!(!needs_exec_bit(&link("site.desktop", "https://example.org/")), "links are followed without the trust question");
}

fn shell(script: &str) -> std::process::Child {
    std::process::Command::new("sh").args(["-c", script]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn().unwrap()
}

#[test]
fn a_program_that_ends_at_once_saying_something_is_reported_with_a_terminal_hint() {
    assert_eq!(watch_start(shell("echo 'usage: tool FILE'; exit 2"), Duration::from_secs(5)).as_deref(), Some("usage: tool FILE"));
    assert_eq!(watch_start(shell("echo 'error: no display' >&2"), Duration::from_secs(5)).as_deref(), Some("error: no display"));
    assert_eq!(watch_start(shell("exit 3"), Duration::from_secs(5)).as_deref(), Some(""));
    let started = Instant::now();
    assert_eq!(watch_start(shell("(sleep 3) & echo started"), Duration::from_secs(5)).as_deref(), Some("started"), "a child keeping the pipe open does not hold Flit");
    assert!(started.elapsed() < Duration::from_secs(2));
    let hint = console_hint("\"tool\"", "usage: tool FILE");
    assert_eq!(hint, flit_core::i18n::S::ErrorsOpsExtraEndedSaying.en().replace("{name}", "\"tool\"").replace("{words}", "usage: tool FILE"));
    assert_eq!(console_hint("\"tool\"", ""), flit_core::i18n::S::ErrorsOpsExtraEndedAtOnce.en().replace("{name}", "\"tool\""));
}

#[test]
fn programs_that_keep_running_or_end_quietly_are_not_reported() {
    let started = Instant::now();
    assert_eq!(watch_start(shell("sleep 2"), Duration::from_millis(200)), None);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(watch_start(shell("true"), Duration::from_secs(5)), None);
    assert_eq!(first_words(&"x".repeat(500).into_bytes()).chars().count(), 161);
}

#[test]
fn quick_view_asks_gnome_sushi_to_toggle_its_preview_of_the_file() {
    let [v2, v1] = sushi_calls(Path::new("/home/u/my photo.jpg"));
    assert_eq!(&v2[..3], ["gdbus", "call", "--session"]);
    assert!(v2.contains(&"org.gnome.NautilusPreviewer2.ShowFile".to_owned()) && v1.contains(&"org.gnome.NautilusPreviewer.ShowFile".to_owned()));
    assert_eq!(v2[v2.len() - 3..], ["file:///home/u/my%20photo.jpg", "''", "true"]);
    assert_eq!(v1[v1.len() - 3..], ["file:///home/u/my%20photo.jpg", "0", "true"]);
}

#[test]
fn without_a_session_bus_quick_view_falls_back_to_flits_own_preview() {
    let t = Temp::new("sushi");
    let file = t.file("a.txt", 3);
    assert!(!session_bus(), "tests run without the desktop's session bus");
    assert!(!LinuxShell.quick_view(&fs_path(&file)));
}

fn app_entry(id: &str, exec: &str, categories: &str) -> crate::apps::Entry {
    let text = format!("[Desktop Entry]\nType=Application\nName={id}\nExec={exec}\nMimeType=inode/directory;\nCategories={categories}\n");
    crate::apps::parse_desktop(id, Path::new(&format!("/usr/share/applications/{id}")), &text, "").unwrap()
}

#[test]
fn the_system_file_manager_is_another_file_manager_than_flit() {
    let entries = [app_entry("flit.desktop", "flit %U", "System;FileManager;"), app_entry("userapp-flit-x.desktop", "/opt/flit/flit %U", "FileManager;"), app_entry("baobab.desktop", "baobab %U", "GTK;System;"), app_entry("org.gnome.Nautilus.desktop", "nautilus --new-window %U", "GNOME;Utility;Core;FileManager;")];
    let mut assoc = crate::apps::Assoc::default();
    crate::apps::parse_mimeapps("[Default Applications]\ninode/directory=flit.desktop;\n", &mut assoc);
    assert_eq!(other_file_manager(&entries, &assoc).map(|e| e.id.as_str()), Some("org.gnome.Nautilus.desktop"));
    assert_eq!(other_file_manager(&entries[..3], &assoc), None, "Flit and a disk usage tool are not offered");
    assert_eq!(loop_device("Mapped file /home/u/disk.iso as /dev/loop7.\n"), Some(PathBuf::from("/dev/loop7")));
    assert_eq!(loop_device("Error setting up loop device"), None);
}

#[test]
fn fonts_install_into_the_users_font_folder_and_other_files_are_refused() {
    let t = Temp::new("fonts");
    let fonts = t.path().join("share/fonts");
    let font = t.file("Sans.ttf", 64);
    let text = t.file("notes.txt", 3);
    let out = install_fonts(&[fs_path(&font), fs_path(&text)], Some(fonts.clone()));
    assert_eq!(out.items, [(fs_path(&font), fs_path(&fonts.join("Sans.ttf")))]);
    assert_eq!(std::fs::read(fonts.join("Sans.ttf")).unwrap().len(), 64);
    assert!(out.error.is_some_and(|e| e.message.contains("notes.txt")), "the text file is named");
    let again = install_fonts(&[fs_path(&fonts.join("Sans.ttf"))], Some(fonts.clone()));
    assert!(again.error.is_none());
    assert_eq!(std::fs::read(fonts.join("Sans.ttf")).unwrap().len(), 64, "installing an installed font does not empty it");
}

#[test]
fn the_desktop_accent_and_text_size_are_read_from_the_portal_gsettings_or_kdeglobals() {
    let accent = parse_accent("(<<(0.20784313725490197, 0.51764705882352946, 0.89411764705882357)>>,)\n").unwrap();
    assert_eq!(accent, flit_core::theme::Accent::single(flit_core::render::Color::rgb(0x3584E4)));
    assert_eq!(kde_accent("[General]\nAccentColor=61,174,233\n"), Some(flit_core::theme::Accent::single(flit_core::render::Color::rgb(0x3DAEE9))));
    assert_eq!(parse_text_scale("(<<1.25>>,)\n"), Some(1.25));
    assert_eq!(parse_text_scale("1.5\n"), Some(1.5));
}

#[test]
fn an_unset_or_odd_accent_and_text_size_keep_flits_own() {
    assert_eq!(parse_accent("(<<(-1.0, -1.0, -1.0)>>,)"), None, "out of range means the accent is unset");
    assert_eq!(parse_accent("Error: GDBus.Error:org.freedesktop.portal.Error.NotFound: Requested setting not found"), None);
    assert_eq!(kde_accent("[General]\nColorScheme=BreezeDark\n"), None);
    assert_eq!(kde_accent("[General]\nAccentColor=300,0,0\n"), None);
    assert_eq!(parse_text_scale("(<<40.0>>,)"), None);
    assert_eq!(parse_text_scale(""), None);
    assert_eq!(crate::window::text_scale(false, 1500), 1.0, "on X11 the desktop already scales text through Xft.dpi");
    assert_eq!(crate::window::text_scale(true, 1500), 1.5);
}
