use super::*;
use crate::temp::{with_tool_dir, Temp};
use std::os::unix::fs::PermissionsExt;

const USER_DIRS_TEXT: &str = "XDG_DESKTOP_DIR=\"$HOME/Desk\"\nXDG_PICTURES_DIR=\"$HOME/Bilder\"\nXDG_DOWNLOAD_DIR=\"$HOME\"\n";

fn theme(name: &str) -> Option<Source> {
    Some(Source::Theme(name.into()))
}

fn png(path: &Path, side: u32) {
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), side, side);
    e.set_color(png::ColorType::Rgba);
    e.write_header().unwrap().write_image_data(&vec![200u8; (side * side * 4) as usize]).unwrap();
}

fn fake_gio(t: &Temp, output: &str) -> PathBuf {
    let tools = t.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let log = t.path().join("calls");
    let script = format!("#!/bin/sh\necho \"$#\" >> '{}'\ncat <<'END'\n{output}END\n", log.display());
    let gio = tools.join("gio");
    std::fs::write(&gio, script).unwrap();
    std::fs::set_permissions(&gio, std::fs::Permissions::from_mode(0o755)).unwrap();
    log
}

fn calls(log: &Path) -> Vec<usize> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(|l| l.trim().parse().unwrap()).collect()
}

#[test]
fn directory_files_name_a_theme_icon_or_an_absolute_picture() {
    assert_eq!(directory_icon(b"[Desktop Entry]\nIcon=folder-red\n"), theme("folder-red"));
    assert_eq!(directory_icon(b"[Desktop Entry]\nIcon = /home/ann/Pictures/my\\sicon.png\n"), Some(Source::Image(PathBuf::from("/home/ann/Pictures/my icon.png"))));
    assert_eq!(directory_icon(b"[Other]\nIcon=wrong\n[Desktop Entry]\nIcon[de]=german\nIcon=folder-blue.svg\n"), theme("folder-blue"));
    assert_eq!(directory_icon(b"Icon=outside\n[Desktop Entry]\nName=x\n"), None);
    assert_eq!(directory_icon(b"[Desktop Entry]\nIcon=\n"), None);
    assert_eq!(directory_icon(b"[Desktop Entry]\nIcon=../escape.png\n"), None);
    assert_eq!(directory_icon(b"[Desktop Entry]\nIcon=bad\\qescape\n"), None);
    assert_eq!(directory_icon(b"[Desktop Entry]\nIcon=\xff\xfe\n"), None);
    assert_eq!(directory_icon(b""), None);
}

#[test]
fn a_folder_with_a_directory_file_gets_its_icon_and_follows_edits() {
    let t = Temp::new("dot-directory");
    let dir = t.path().join("Projects");
    std::fs::create_dir(&dir).unwrap();
    assert!(sources(&dir).is_empty());
    std::fs::write(dir.join(".directory"), "[Desktop Entry]\nIcon=folder-red\n").unwrap();
    assert_eq!(sources(&dir), vec![Source::Theme("folder-red".into())]);
    std::fs::write(dir.join(".directory"), "[Desktop Entry]\nIcon=folder-tan\n").unwrap();
    assert_eq!(sources(&dir), vec![Source::Theme("folder-tan".into())], "an edit of the same length is noticed");
    std::fs::write(dir.join(".directory"), "[Desktop Entry]\nIcon=broken\\").unwrap();
    assert!(sources(&dir).is_empty());
    std::fs::remove_file(dir.join(".directory")).unwrap();
    assert!(sources(&dir).is_empty());
}

#[test]
fn oversized_or_special_directory_files_are_ignored_without_blocking() {
    let t = Temp::new("dot-directory-odd");
    let big = t.path().join("big");
    std::fs::create_dir(&big).unwrap();
    let mut text = b"[Desktop Entry]\nIcon=folder-red\n".to_vec();
    text.resize(70 * 1024, b'\n');
    std::fs::write(big.join(".directory"), text).unwrap();
    assert!(sources(&big).is_empty());
    let fifo = t.path().join("fifo");
    std::fs::create_dir(&fifo).unwrap();
    let name = std::ffi::CString::new(fifo.join(".directory").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
    let started = Instant::now();
    assert!(sources(&fifo).is_empty());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(icon(&t.path().join("missing"), 32).is_none());
}

#[test]
fn a_picture_named_by_the_folder_is_drawn_and_a_missing_one_is_skipped() {
    let t = Temp::new("dot-directory-picture");
    let dir = t.path().join("Album");
    std::fs::create_dir(&dir).unwrap();
    png(&t.path().join("cover.png"), 64);
    std::fs::write(dir.join(".directory"), format!("[Desktop Entry]\nIcon={}\n", t.path().join("cover.png").display())).unwrap();
    let made = icon(&dir, 32).expect("the picture becomes the folder icon");
    assert_eq!(made.width.max(made.height), 32);
    std::fs::write(dir.join(".directory"), format!("[Desktop Entry]\nIcon={}\n", t.path().join("gone.png").display())).unwrap();
    assert!(icon(&dir, 32).is_none(), "nothing custom is left, so the shared folder icon is used");
}

#[test]
fn only_the_configured_user_folders_get_their_list_icons() {
    let home = PathBuf::from("/home/ann");
    let dirs = crate::xdg::user_dirs_from(USER_DIRS_TEXT.as_bytes(), &home);
    let names = |p: &str| xdg_names(Path::new(p), Some(&home), &dirs);
    assert_eq!(names("/home/ann/Bilder"), ["folder-pictures"]);
    assert_eq!(names("/home/ann/Desk"), ["folder-desktop", "user-desktop"]);
    assert_eq!(names("/home/ann/Documents"), ["folder-documents"]);
    assert_eq!(names("/home/ann/Music"), ["folder-music"]);
    assert_eq!(names("/home/ann/Videos"), ["folder-videos"]);
    assert_eq!(names("/home/ann/Templates"), ["folder-templates"]);
    assert_eq!(names("/home/ann/Public"), ["folder-publicshare"]);
    assert!(names("/home/ann/Pictures").is_empty(), "the default name is not used once another folder is configured");
    assert!(names("/media/usb/Bilder").is_empty(), "a folder of the same name elsewhere stays plain");
    assert!(names("/home/ann").is_empty(), "a user folder set to home does not restyle home");
    assert!(names("/home/ann/Bilder/2024").is_empty());
}

#[test]
fn the_home_xdg_folders_of_this_session_are_found_with_the_custom_icon_first() {
    let home = crate::xdg::home().unwrap();
    let pictures = home.join("Pictures");
    std::fs::create_dir_all(&pictures).unwrap();
    assert!(sources(&pictures).contains(&Source::Theme("folder-pictures".into())));
    let t = Temp::new("xdg-elsewhere");
    let other = t.path().join("Pictures");
    std::fs::create_dir(&other).unwrap();
    assert!(sources(&other).is_empty());
    std::fs::write(other.join(".directory"), "[Desktop Entry]\nIcon=folder-red\n").unwrap();
    assert_eq!(sources(&other), vec![Source::Theme("folder-red".into())]);
}

#[test]
fn gio_output_for_several_folders_is_matched_by_uri() {
    let asked = vec![PathBuf::from("/home/ann/a b"), PathBuf::from("/home/ann/plain"), PathBuf::from("/home/ann/named")];
    let out = b"uri: file:///home/ann/a%20b\nlocal path: /home/ann/a b\nunix mount: /dev/sda1 / ext4 rw\nattributes:\n  metadata::custom-icon: file:///home/ann/My%20Pic.png\nuri: file:///home/ann/plain\nlocal path: /home/ann/plain\nattributes:\nuri: file:///home/ann/named\nlocal path: /home/ann/named\nattributes:\n  metadata::custom-icon: folder-red\n";
    assert_eq!(parse_gio(out, &asked), vec![Some(Source::Image(PathBuf::from("/home/ann/My Pic.png"))), None, theme("folder-red")]);
}

#[test]
fn unexpected_gio_output_never_lends_an_icon_to_another_folder() {
    let asked = vec![PathBuf::from("/a"), PathBuf::from("/b")];
    assert_eq!(parse_gio(b"garbage\n\x00\xff\n  metadata::custom-icon: folder-red\n", &asked), vec![None, None]);
    assert_eq!(parse_gio(b"uri: file:///other\n  metadata::custom-icon: folder-red\nuri: file:///b\nattributes:\n", &asked), vec![None, None]);
    assert_eq!(parse_gio(b"uri: file:///a\n  metadata::custom-icon: one\nuri: file:///a\n  metadata::custom-icon: two\n", &asked), vec![None, None], "a folder reported twice is ambiguous");
    assert_eq!(parse_gio(b"uri: file:///a\n  metadata::custom-icon: https://example.com/x.png\nuri: file:///b\n  metadata::custom-icon: smb://server/x.png\n", &asked), vec![None, None]);
    assert_eq!(parse_gio(b"uri: file:///b\n  metadata::custom-icon: file:///pic.png?x\n", &asked), vec![None, None]);
    assert_eq!(parse_gio(b"uri: sftp://host/a\n  metadata::custom-icon: folder-red\n", &asked), vec![None, None]);
    assert_eq!(parse_gio(b"uri: file:///b\n  metadata::custom-icon: folder\\x2dred\n", &asked), vec![None, theme("folder-red")]);
    assert_eq!(parse_gio(b"", &asked), vec![None, None]);
}

#[test]
fn visible_folders_are_asked_in_batches_of_at_most_a_hundred_and_remembered() {
    let t = Temp::new("gio-batch");
    let dirs: Vec<PathBuf> = (0..150).map(|i| t.path().join(format!("d{i:03}"))).collect();
    let picked = &dirs[3];
    let log = fake_gio(&t, &format!("uri: file://{}\nattributes:\n  metadata::custom-icon: folder-green\n", picked.display()));
    with_tool_dir(&t.path().join("tools"), || {
        prepare(&dirs);
        assert_eq!(calls(&log), [104, 54], "two calls, each with the fixed arguments and at most 100 folders");
        prepare(&dirs[..10]);
        assert_eq!(calls(&log).len(), 2, "known folders do not start gio again");
        assert_eq!(gvfs(picked), theme("folder-green"));
        assert_eq!(gvfs(&dirs[4]), None);
        assert_eq!(calls(&log).len(), 2);
    });
}

#[test]
fn without_gio_custom_icons_are_quietly_skipped() {
    let t = Temp::new("gio-missing");
    let dir = t.path().join("x");
    std::fs::create_dir(&dir).unwrap();
    with_tool_dir(&t.path().join("no-tools"), || {
        prepare(std::slice::from_ref(&dir));
        assert!(sources(&dir).is_empty());
    });
}

#[test]
fn a_hanging_gio_is_stopped() {
    let t = Temp::new("gio-hang");
    let gio = t.path().join("gio");
    std::fs::write(&gio, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&gio, std::fs::Permissions::from_mode(0o755)).unwrap();
    let started = Instant::now();
    assert!(run_gio(&gio, &[t.path().to_path_buf()]).is_none());
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn the_cache_stays_bounded() {
    let t = Temp::new("bounded");
    let dirs: Vec<PathBuf> = (0..KEEP + 200).map(|i| t.path().join(format!("{i}"))).collect();
    with_tool_dir(&t.path().join("no-tools"), || prepare(&dirs));
    assert!(remembered_count() <= KEEP);
    assert!(remembered(dirs.last().unwrap()));
    forget(dirs.last().unwrap());
    assert!(!remembered(dirs.last().unwrap()));
}

#[test]
fn the_shell_answers_folder_requests_only_with_a_folders_own_icon() {
    use flit_platform::Shell;
    let t = Temp::new("shell-folder");
    let (own, plain) = (t.path().join("own"), t.path().join("plain"));
    std::fs::create_dir(&own).unwrap();
    std::fs::create_dir(&plain).unwrap();
    png(&t.path().join("cover.png"), 64);
    std::fs::write(own.join(".directory"), format!("[Desktop Entry]\nIcon={}\n", t.path().join("cover.png").display())).unwrap();
    let req = |p: &Path| flit_core::ui::IconRequest { key: 1, kind: flit_core::ui::IconKind::CustomFolder, path: crate::xdg::fs_path(p).unwrap(), size: 32, gen: 0, row: 0 };
    let mut shell = crate::shell::LinuxShell::new().unwrap();
    shell.prepare_icons(&[req(&own), req(&plain)]);
    assert!(shell.icon(&req(&own)).is_some());
    assert!(shell.icon(&req(&plain)).is_none(), "a plain folder leaves the shared folder icon to the list");
}
