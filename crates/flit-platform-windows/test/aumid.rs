use super::*;

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("flit-aumid-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn make_link(link: &Path, target: &Path, id: Option<&str>) {
    crate::com_sta();
    unsafe {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
        shell.SetPath(PCWSTR(wide(target).as_ptr())).unwrap();
        if let Some(id) = id {
            let store: IPropertyStore = shell.cast().unwrap();
            store.SetValue(&PKEY_APP_ID, &PROPVARIANT::from(id)).unwrap();
            store.Commit().unwrap();
        }
        shell.cast::<IPersistFile>().unwrap().Save(PCWSTR(wide(link).as_ptr()), true).unwrap();
    }
}

fn fake_exe(dir: &Path, name: &str) -> PathBuf {
    let exe = dir.join("bin").join(name);
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"MZ").unwrap();
    exe
}

#[test]
fn shortcuts_to_this_flit_in_start_menu_taskbar_and_desktop_get_the_application_identity() {
    let d = Dir::new("fix");
    let exe = fake_exe(&d.0, "flit.exe");
    let start = d.0.join(r"Start Menu\Programs\Flit");
    let taskbar = d.0.join(r"User Pinned\TaskBar");
    let desktop = d.0.join("Desktop");
    for dir in [&start, &taskbar, &desktop] {
        std::fs::create_dir_all(dir).unwrap();
    }
    make_link(&start.join("Flit.lnk"), &exe, None);
    make_link(&taskbar.join("Flit.lnk"), &exe, Some("Someone.Else"));
    make_link(&desktop.join("Flit.lnk"), &exe, None);
    let roots = [d.0.join("Start Menu"), taskbar.clone(), desktop.clone()];
    let outcome = fix(&roots, &exe, &mut ShellLinks);
    assert_eq!(outcome.fixed.len(), 3, "{outcome:?}");
    assert!(outcome.failed.is_empty());
    for link in [start.join("Flit.lnk"), taskbar.join("Flit.lnk"), desktop.join("Flit.lnk")] {
        let (target, id) = ShellLinks.inspect(&link).unwrap();
        assert_eq!(id.as_deref(), Some(APP_ID), "{}", link.display());
        assert!(same_exe(&target, &exe), "the shortcut still starts the same program");
    }
    let leftovers: Vec<_> = std::fs::read_dir(&start).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(leftovers.len(), 1, "no temporary file is left next to the shortcut: {leftovers:?}");
}

#[test]
fn shortcuts_to_other_programs_and_ones_already_right_are_left_untouched() {
    let d = Dir::new("keep");
    let exe = fake_exe(&d.0, "flit.exe");
    let other = fake_exe(&d.0.join("other"), "notflit.exe");
    let elsewhere = fake_exe(&d.0.join("copy"), "flit.exe");
    let links = d.0.join("Links");
    std::fs::create_dir_all(&links).unwrap();
    make_link(&links.join("Other.lnk"), &other, None);
    make_link(&links.join("Right.lnk"), &exe, Some(APP_ID));
    make_link(&links.join("Copy.lnk"), &elsewhere, None);
    std::fs::write(links.join("Fake.lnk"), b"flit.exe but not a shortcut").unwrap();
    let before: Vec<_> = ["Other.lnk", "Right.lnk", "Copy.lnk", "Fake.lnk"].iter().map(|n| std::fs::read(links.join(n)).unwrap()).collect();
    let outcome = fix(std::slice::from_ref(&links), &exe, &mut ShellLinks);
    assert!(outcome.fixed.is_empty() && outcome.failed.is_empty(), "{outcome:?}");
    let after: Vec<_> = ["Other.lnk", "Right.lnk", "Copy.lnk", "Fake.lnk"].iter().map(|n| std::fs::read(links.join(n)).unwrap()).collect();
    assert_eq!(before, after);
}

#[test]
fn a_read_only_shortcut_is_skipped_and_reported_without_changes() {
    let d = Dir::new("ro");
    let exe = fake_exe(&d.0, "flit.exe");
    let link = d.0.join("Flit.lnk");
    make_link(&link, &exe, None);
    let original = std::fs::metadata(&link).unwrap().permissions();
    let mut perms = original.clone();
    perms.set_readonly(true);
    std::fs::set_permissions(&link, perms).unwrap();
    let before = std::fs::read(&link).unwrap();
    let outcome = fix(std::slice::from_ref(&d.0), &exe, &mut ShellLinks);
    assert!(outcome.fixed.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert_eq!(std::fs::read(&link).unwrap(), before);
    std::fs::set_permissions(&link, original).unwrap();
}

struct Broken(Vec<PathBuf>);

impl Links for Broken {
    fn inspect(&mut self, _: &Path) -> Result<(PathBuf, Option<String>), String> {
        Ok((PathBuf::from(r"C:\Apps\Flit\flit.exe"), None))
    }
    fn write(&mut self, link: &Path) -> Result<(), String> {
        self.0.push(link.to_owned());
        Err("access denied".into())
    }
}

#[test]
fn a_shortcut_that_cannot_be_written_is_reported_and_the_rest_continue() {
    let d = Dir::new("deny");
    for name in ["a.lnk", "b.LNK"] {
        std::fs::write(d.0.join(name), "C:\\Apps\\Flit\\FLIT.EXE".encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>()).unwrap();
    }
    let mut links = Broken(Vec::new());
    let outcome = fix(std::slice::from_ref(&d.0), Path::new(r"C:\Apps\Flit\flit.exe"), &mut links);
    assert_eq!(links.0.len(), 2);
    assert_eq!(outcome.failed.len(), 2);
}

#[test]
fn deciding_which_shortcut_needs_the_identity() {
    let exe = Path::new(r"C:\Apps\Flit\flit.exe");
    assert!(needs_id(Path::new(r"c:\apps\flit\FLIT.EXE"), None, exe));
    assert!(needs_id(Path::new(r"\\?\C:\Apps\Flit\flit.exe"), Some("Other.App"), exe));
    assert!(!needs_id(exe, Some(APP_ID), exe));
    assert!(!needs_id(Path::new(r"C:\Apps\Other\flit.exe"), None, exe));
    assert!(!needs_id(Path::new(""), None, exe));
}

#[test]
fn the_cheap_filter_finds_the_exe_name_in_narrow_and_wide_text() {
    assert!(mentions(b"..C:\\APPS\\FLIT.EXE..", "flit.exe"));
    let wide: Vec<u8> = "C:\\Apps\\Flit.exe".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert!(mentions(&wide, "flit.exe"));
    assert!(!mentions(b"notepad.exe", "flit.exe"));
    assert!(mentions(b"anything", "\u{444}\u{43b}\u{438}\u{442}.exe"), "a non-ASCII name is not filtered out early");
}

#[test]
fn only_shortcut_files_are_collected_and_nested_folders_are_searched() {
    let d = Dir::new("walk");
    std::fs::create_dir_all(d.0.join(r"a\b")).unwrap();
    for f in [r"x.lnk", r"a\y.LNK", r"a\b\z.lnk", r"a\notes.txt", r"a\b\w.url"] {
        std::fs::write(d.0.join(f), b"").unwrap();
    }
    let mut found: Vec<String> = walk(std::slice::from_ref(&d.0)).0.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    found.sort();
    assert_eq!(found, ["x.lnk", "y.LNK", "z.lnk"]);
    assert!(walk(&[d.0.join("missing")]).0.is_empty());
}

const ARGS: &str = r#"--new-window "D:\Some Folder""#;

fn text(read:impl FnOnce(&mut [u16]) -> windows::core::Result<()>) -> String {
    let mut buf = vec![0u16; 1024];
    read(&mut buf).unwrap();
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

#[test]
fn writing_the_identity_keeps_arguments_icon_folder_and_run_as_administrator() {
    use windows::Win32::UI::Shell::{IShellLinkDataList, SLDF_RUNAS_USER};
    let d = Dir::new("keep-props");
    let exe = fake_exe(&d.0, "flit.exe");
    let link = d.0.join("Flit.lnk");
    let work = d.0.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let icon = d.0.join("icon.ico");
    crate::com_sta();
    unsafe {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
        shell.SetPath(PCWSTR(wide(&exe).as_ptr())).unwrap();
        shell.SetArguments(PCWSTR(wide(Path::new(ARGS)).as_ptr())).unwrap();
        shell.SetWorkingDirectory(PCWSTR(wide(&work).as_ptr())).unwrap();
        shell.SetIconLocation(PCWSTR(wide(&icon).as_ptr()), 3).unwrap();
        let data: IShellLinkDataList = shell.cast().unwrap();
        data.SetFlags(data.GetFlags().unwrap() | SLDF_RUNAS_USER.0 as u32).unwrap();
        shell.cast::<IPersistFile>().unwrap().Save(PCWSTR(wide(&link).as_ptr()), true).unwrap();
    }
    let outcome = fix(std::slice::from_ref(&d.0), &exe, &mut ShellLinks);
    assert_eq!(outcome.fixed, std::slice::from_ref(&link));
    unsafe {
        let shell = ShellLinks::load(&link, STGM_READ).unwrap();
        assert_eq!(text(|b| shell.GetArguments(b)), ARGS);
        assert_eq!(PathBuf::from(text(|b| shell.GetWorkingDirectory(b))), work);
        let mut index = 0;
        assert_eq!(PathBuf::from(text(|b| shell.GetIconLocation(b, &mut index))), icon);
        assert_eq!(index, 3);
        let data: IShellLinkDataList = shell.cast().unwrap();
        assert_ne!(data.GetFlags().unwrap() & SLDF_RUNAS_USER.0 as u32, 0, "run as administrator stays on");
    }
    assert_eq!(ShellLinks.inspect(&link).unwrap().1.as_deref(), Some(APP_ID));
}

#[test]
fn leftover_temporary_shortcut_copies_are_removed_and_other_files_stay() {
    let d = Dir::new("leftover");
    let exe = fake_exe(&d.0, "flit.exe");
    std::fs::write(d.0.join("Flit.lnk.flit-1234.tmp"), b"x").unwrap();
    std::fs::write(d.0.join("notes.flit-1234.tmp"), b"x").unwrap();
    std::fs::write(d.0.join("Flit.lnk.flit-abc.tmp"), b"x").unwrap();
    let _ = fix(std::slice::from_ref(&d.0), &exe, &mut ShellLinks);
    assert!(!d.0.join("Flit.lnk.flit-1234.tmp").exists());
    assert!(d.0.join("notes.flit-1234.tmp").exists());
    assert!(d.0.join("Flit.lnk.flit-abc.tmp").exists());
}
