use super::*;
use flit_core::provider::FileOp;
use flit_core::ui::IconRequest;
use flit_platform::FileOps;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

#[test]
fn launch_notification_is_sent_only_when_enabled_and_spawn_succeeds() {
    let program = std::env::var("SystemRoot").unwrap() + r"\System32\where.exe";
    for enabled in [false, true] {
        let mut notices = Vec::new();
        let runs = vec![(vec![program.clone(), "/q".into(), "where.exe".into()], vec![("FLIT_STARTUP_NOTIFY".into(), if enabled { "1" } else { "0" }.into())])];
        launch_commands(&runs, None, |text| notices.push(text.to_owned())).unwrap();
        assert_eq!(notices.len(), usize::from(enabled));
        if enabled { assert!(notices[0].contains("where.exe")); }
    }
    let mut notices = Vec::new();
    let missing = vec![(vec![r"C:\flit-missing-notification-test.exe".into()], vec![("FLIT_STARTUP_NOTIFY".into(), "1".into())])];
    assert!(launch_commands(&missing, None, |text| notices.push(text.to_owned())).is_err());
    assert!(notices.is_empty(), "a failed launch must never report success");
}

#[test]
fn cancelled_system_launches_are_not_successful_starts() {
    assert!(!launch_accepted(Err(windows::core::Error::from_hresult(ERROR_CANCELLED.to_hresult()))).unwrap());
    assert!(launch_accepted(Ok(())).unwrap());
    assert!(launch_accepted(Err(windows::core::Error::from_hresult(windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.to_hresult()))).is_err());
}

#[test]
fn argv_commands_report_spawn_errors_instead_of_silently_succeeding() {
    let mut shell = ShellCom::new().unwrap();
    let command = Launch::Command { runs: vec![(vec![r"C:\flit-nonexistent-action-program.exe".into(), "a & b; $(literal)".into()], Vec::new())], dir: None };
    assert!(shell.launch(&command).is_err());
}

#[test]
fn shortcuts_resolve_only_to_existing_folders() {
    let root = crate::test_temp().join(format!("flit-shell-lnk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("f.txt"), "x").unwrap();
    let at = |rel: &str| p(root.join(rel).to_str().unwrap());
    let mut sh = ShellCom::new().unwrap();
    let out = sh.file_op(&FileOp::Link { items: vec![at("target"), at("f.txt")], dest: at("") }, &flit_platform::Job::detached(flit_core::provider::Resolve::Skip));
    let [(_, to_folder), (_, to_file)] = out.items.try_into().unwrap();
    assert_eq!(sh.shortcut_folder(&to_folder), Some(at("target")));
    assert_eq!(sh.shortcut_folder(&to_file), None);
    assert_eq!(sh.shortcut_folder(&at("f.txt")), None);
    std::fs::remove_dir(root.join("target")).unwrap();
    assert_eq!(sh.shortcut_folder(&to_folder), None, "a missing target is left to the shell");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn terminal_commands_open_one_tab_per_folder_with_quoted_paths() {
    let dirs = vec![p(r"C:\a b"), p(r"C:\"), p(r"D:\x;y"), p(r"\\wsl.localhost\Ubuntu\home\u")];
    let tabs = r#"new-tab -d "C:\a b" ; new-tab -d C:\ ; new-tab -d D:\x\;y ; new-tab wsl.exe -d Ubuntu --cd /home/u"#;
    let wt = commands(&Launch::Terminal { dirs: dirs.clone(), admin: true }, Some(r"C:\wt.exe"));
    assert_eq!(wt, [Command { program: r"C:\wt.exe".into(), args: tabs.into(), dir: None, admin: true }]);
    let consoles = commands(&Launch::Terminal { dirs, admin: false }, None);
    let shown: Vec<(&str, &str)> = consoles.iter().map(|c| (c.program.as_str(), c.args.as_str())).collect();
    assert_eq!(shown, [("cmd.exe", r#"/k pushd "C:\a b""#), ("cmd.exe", r#"/k pushd "C:\""#), ("cmd.exe", r#"/k pushd "D:\x;y""#), ("wsl.exe", "-d Ubuntu --cd /home/u")]);
    assert!(consoles.iter().all(|c| !c.admin && c.dir.is_none()));
}

#[test]
fn dropped_files_become_quoted_program_arguments() {
    let launch = Launch::With { program: p(r"C:\tools\run.cmd"), args: vec![p(r"D:\in\a b.txt"), p(r"D:\in\c.txt"), p(r"E:\")] };
    let expected = Command { program: r"C:\tools\run.cmd".into(), args: r#""D:\in\a b.txt" D:\in\c.txt E:\"#.into(), dir: Some(r"C:\tools".into()), admin: false };
    assert_eq!(commands(&launch, Some(r"C:\wt.exe")), [expected]);
    assert_eq!(quote_path(r"C:\My Folder\"), r#""C:\My Folder\\""#);
    assert_eq!(quote_path(""), r#""""#);
}

#[test]
fn typed_commands_run_as_typed_and_eject_needs_no_command() {
    let run = Launch::Run { program: "notepad".into(), args: r#""a b.txt" -x"#.into(), dir: Some(p(r"C:\work")) };
    let expected = Command { program: "notepad".into(), args: r#""a b.txt" -x"#.into(), dir: Some(r"C:\work".into()), admin: false };
    assert_eq!(commands(&run, None), [expected]);
    let url = Launch::Run { program: "https://example.com".into(), args: String::new(), dir: None };
    assert_eq!(commands(&url, None).first().map(|c| (c.program.as_str(), c.dir.is_none())), Some(("https://example.com", true)));
    assert!(commands(&Launch::Eject(p(r"E:\")), None).is_empty());
}

fn solid(size: u32, px: [u8; 4]) -> Bitmap {
    Bitmap { width: size, height: size, rgba: px.repeat((size * size) as usize) }
}

#[test]
fn fit_never_exceeds_the_requested_size() {
    let b = fit(solid(256, [10, 20, 30, 40]), 56);
    assert_eq!((b.width, b.height, b.rgba.len()), (56, 56, 56 * 56 * 4));
    assert!(b.rgba.chunks_exact(4).all(|p| p == [10, 20, 30, 40]));
    let small = fit(solid(48, [1, 2, 3, 4]), 56);
    assert_eq!((small.width, small.height), (48, 48));
}

#[test]
fn fit_box_filters_premultiplied_pixels() {
    let mut b = solid(4, [0, 0, 0, 0]);
    for y in 0..2 {
        for x in 0..2 {
            let p = ((y * 4 + x) * 4) as usize;
            b.rgba[p..p + 4].copy_from_slice(&[200, 100, 0, 200]);
        }
    }
    let half = fit(b, 2);
    assert_eq!(&half.rgba[..4], &[200, 100, 0, 200]);
    assert_eq!(&half.rgba[4..8], &[0, 0, 0, 0]);
    let quarter = fit(half, 1);
    assert_eq!(quarter.rgba, vec![50, 25, 0, 50]);
}

#[test]
fn unpad_crops_a_small_icon_parked_in_the_top_left_corner_of_a_jumbo_canvas() {
    let mut b = solid(256, [0, 0, 0, 0]);
    for y in 0..16 {
        for x in 0..16 {
            let p = ((y * 256 + x) * 4) as usize;
            b.rgba[p..p + 4].copy_from_slice(&[9, 9, 9, 255]);
        }
    }
    let cropped = unpad(b);
    assert_eq!((cropped.width, cropped.height), (16, 16));
    assert!(cropped.rgba.chunks_exact(4).all(|p| p == [9, 9, 9, 255]));
    let full = unpad(solid(48, [1, 1, 1, 255]));
    assert_eq!((full.width, full.height), (48, 48));
    let mut centered = solid(64, [0, 0, 0, 0]);
    let p = ((32 * 64 + 32) * 4) as usize;
    centered.rgba[p..p + 4].copy_from_slice(&[5, 5, 5, 255]);
    assert_eq!(unpad(centered).width, 64);
    assert_eq!(unpad(solid(32, [0, 0, 0, 0])).width, 32);
}

pub(crate) fn write_jpeg(path: &std::path::Path, width: u32, height: u32, seed: u32) {
    use windows::Win32::Graphics::Imaging::*;
    let name: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let mut px = vec![0u8; (width * height * 3) as usize];
    for (k, p) in px.chunks_exact_mut(3).enumerate() {
        let (x, y) = (k as u32 % width, k as u32 / width);
        let n = (x.wrapping_mul(7919) ^ y.wrapping_mul(104_729) ^ seed.wrapping_mul(2_654_435_761)).wrapping_mul(2_246_822_519) >> 27;
        p.copy_from_slice(&[(x * 255 / width) as u8 ^ n as u8, (y * 255 / height) as u8, (seed * 37) as u8 ^ (n as u8) << 2]);
    }
    com_sta();
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let stream = wic.CreateStream().unwrap();
        stream.InitializeFromFilename(PCWSTR(name.as_ptr()), 0x4000_0000).unwrap();
        let enc = wic.CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null()).unwrap();
        enc.Initialize(&stream, WICBitmapEncoderNoCache).unwrap();
        let (mut frame, mut props) = (None, None);
        enc.CreateNewFrame(&mut frame, &mut props).unwrap();
        let frame = frame.unwrap();
        frame.Initialize(props.as_ref()).unwrap();
        frame.SetSize(width, height).unwrap();
        let mut format = GUID_WICPixelFormat24bppBGR;
        frame.SetPixelFormat(&mut format).unwrap();
        frame.WritePixels(height, width * 3, &px).unwrap();
        frame.Commit().unwrap();
        enc.Commit().unwrap();
    }
}

#[test]
fn thumbnails_of_images_fit_the_requested_size_and_other_files_have_none() {
    let root = crate::test_temp().join(format!("flit-shell-thumb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut shell = ShellCom::new().unwrap();
    write_jpeg(&root.join("photo.jpg"), 320, 200, 1);
    std::fs::write(root.join("notes.txt"), "x").unwrap();
    let req = |name: &str, size| IconRequest { key: 0, kind: IconKind::Thumbnail, path: p(root.join(name).to_str().unwrap()), size, gen: 0, row: 0 };
    for size in [64, 200] {
        let b = shell.thumbnail(&req("photo.jpg", size)).expect("jpeg thumbnail");
        assert!(b.width <= size && b.height <= size && b.width.max(b.height) >= size * 9 / 10, "{size}: {}x{}", b.width, b.height);
        assert!(b.rgba.chunks_exact(4).all(|p| p[3] == 255), "opaque");
    }
    assert!(shell.thumbnail(&req("notes.txt", 64)).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn files_that_are_not_on_the_device_only_get_cached_thumbnails_and_fast_properties() {
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_OFFLINE};
    assert_eq!((thumbnail_flags(false), thumbnail_flags(true)), (SIIGBF_THUMBNAILONLY, SIIGBF_THUMBNAILONLY | SIIGBF_INCACHEONLY));
    assert_eq!((icon_flags(false), icon_flags(true)), (SIIGBF_ICONONLY, SIIGBF_ICONONLY | SIIGBF_INCACHEONLY));
    assert_eq!(crate::preview::store_flags(true), windows::Win32::UI::Shell::PropertiesSystem::GPS_FASTPROPERTIESONLY);
    let root = crate::test_temp().join(format!("flit-shell-offline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("away.jpg");
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() % 100_000 + 2;
    write_jpeg(&file, 300, 180, seed);
    let wide: Vec<u16> = file.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let set = |a| unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), a) }.is_ok();
    let mut shell = ShellCom::new().unwrap();
    let req = IconRequest { key: 0, kind: IconKind::Thumbnail, path: p(file.to_str().unwrap()), size: 96, gen: 0, row: 0 };
    if set(FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_ARCHIVE) {
        assert!(crate::fs::not_local(&req.path));
        assert!(shell.thumbnail(&req).is_none(), "an uncached thumbnail of an offline file is not extracted");
        assert!(set(FILE_ATTRIBUTE_ARCHIVE));
    }
    assert!(!crate::fs::not_local(&req.path));
    assert!(shell.thumbnail(&req).is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn distro_icons_come_from_their_start_menu_shortcut() {
    let mut shell = ShellCom::new().unwrap();
    let req = |distro: &str| IconRequest { key: 0, kind: IconKind::Volume, path: p(&format!(r"\\wsl.localhost\{distro}\")), size: 32, gen: 0, row: 0 };
    assert!(shell.icon(&req("flit-no-such-distro")).is_none());
    let shortcut = |d: &String| crate::places::wsl_shortcut(d).is_some_and(|l| std::path::Path::new(&*l.display()).exists());
    for distro in crate::places::wsl_distros().iter().filter(|d| shortcut(d)) {
        assert!(shell.icon(&req(distro)).is_some_and(|b| b.width <= 32 && b.height <= 32), "{distro}");    }
}

#[test]
fn places_get_their_own_shell_icons_not_the_plain_folder() {
    let mut shell = ShellCom::new().unwrap();
    let req = |kind, path: FsPath| IconRequest { key: 0, kind, path, size: 16, gen: 0, row: 0 };
    let folder = shell.icon(&req(IconKind::Folder, p(r"C:\Windows"))).expect("folder icon").rgba;
    let mut places = vec![FsPath::recycle_bin(), FsPath::computer(), FsPath::home_page()];
    places.extend(std::env::var("USERPROFILE").ok().map(|home| p(&format!(r"{home}\Downloads"))).filter(|d| std::path::Path::new(&*d.display()).is_dir()));
    for place in places {
        let b = shell.icon(&req(IconKind::Place, place.clone())).unwrap_or_else(|| panic!("{place:?}"));
        assert!(b.width <= 16 && b.height <= 16, "{place:?}");
        assert_ne!(b.rgba, folder, "{place:?} has its own icon");
    }
}

#[test]
fn shell_icons_match_requested_size() {
    let mut shell = ShellCom::new().unwrap();
    for size in [16, 56, 100, 300] {
        let req = IconRequest { key: 0, kind: IconKind::Type, path: FsPath::parse(r"C:\x.txt").unwrap(), size, gen: 0, row: 0 };
        let b = shell.icon(&req).expect("type icon");
        assert!(b.width <= size && b.height <= size, "{size}: {}x{}", b.width, b.height);
        assert_eq!(b.rgba.len(), (b.width * b.height * 4) as usize);
    }
}

#[test]
fn quoted_arguments_come_back_whole_through_command_line_to_argv() {
    let args: Vec<String> = [r"C:\plain", "a b", "", r"C:\My Folder\", r#"say "hi""#, r#"a\"b"#, r"tail\\", "tab\there", "%PATH% & echo ^x"].iter().map(|s| (*s).to_owned()).collect();
    let line = format!("x.exe {}", flit_core::cmdline::join(&args, false));
    let wide: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
    let mut n = 0;
    let argv = unsafe { CommandLineToArgvW(PCWSTR(wide.as_ptr()), &mut n) };
    let parsed: Vec<String> = (1..n as usize).map(|k| unsafe { (*argv.add(k)).to_string().unwrap() }).collect();
    unsafe {
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(argv as _)));
    }
    assert_eq!(parsed, args, "{line}");
}

#[test]
fn desktop_ini_names_show_only_in_read_only_or_system_folders_and_resolve_resources() {
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_READONLY};
    let root = crate::test_temp().join(format!("flit-shell-label-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("plain")).unwrap();
    std::fs::write(root.join("plain").join("desktop.ini"), "[.ShellClassInfo]\r\nLocalizedResourceName=Фото\r\n").unwrap();
    let dir = p(root.join("plain").to_str().unwrap());
    assert_eq!(localized_name(&dir), None, "Explorer ignores desktop.ini in ordinary folders");
    let wide: Vec<u16> = root.join("plain").as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_READONLY) }.unwrap();
    assert_eq!(localized_name(&dir).as_deref(), Some("Фото"));
    let documents = resolve_name(r"@%SystemRoot%\system32\shell32.dll,-21770");
    assert!(documents.is_some_and(|d| !d.is_empty() && !d.starts_with('@')));
    unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_NORMAL) }.unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn run_in_terminal_starts_the_program_in_its_folder_and_keeps_the_console() {
    let run = |path: &str, wt: Option<&str>| commands(&Launch::RunInTerminal { program: p(path) }, wt);
    let console = Command { program: "cmd.exe".into(), args: r#"/k call "C:\my tools\build.cmd""#.into(), dir: Some(r"C:\my tools".into()), admin: false };
    assert_eq!(run(r"C:\my tools\build.cmd", None), [console]);
    let tab = Command { program: r"C:\wt.exe".into(), args: r#"new-tab -d "C:\my tools" cmd.exe /k call "C:\my tools\a\;b.exe""#.into(), dir: None, admin: false };
    assert_eq!(run(r"C:\my tools\a;b.exe", Some(r"C:\wt.exe")), [tab]);
    let script = Command { program: "powershell.exe".into(), args: r#"-NoLogo -NoExit -File "D:\s\setup.PS1""#.into(), dir: Some(r"D:\s".into()), admin: false };
    assert_eq!(run(r"D:\s\setup.PS1", None), [script]);
    let linux = run(r"\\wsl.localhost\Ubuntu\home\u\my tool", Some(r"C:\wt.exe"));
    let expected = format!(r#"-d Ubuntu --cd /home/u --exec sh -c {} flit /home/u "/home/u/my tool""#, flit_core::cmdline::quote_windows(WSL_RUN_SCRIPT));
    assert_eq!(linux, [Command { program: "wsl.exe".into(), args: expected, dir: None, admin: false }]);
}

#[test]
fn only_files_that_were_opened_go_to_the_recent_list() {
    let dir = crate::test_temp().join(format!("flit-recent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("note.txt"), b"x").unwrap();
    let at = |name: &str| FsPath::parse(&dir.join(name).to_string_lossy()).unwrap();
    assert!(recent_worthy(&at("note.txt")));
    assert!(!recent_worthy(&FsPath::parse(&dir.to_string_lossy()).unwrap()), "a folder is not a recent document");
    assert!(!recent_worthy(&at("gone.txt")));
    assert!(!recent_worthy(&FsPath::computer()));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_badge_is_drawn_over_the_bottom_left_corner_and_leaves_the_rest_alone() {
    let base = solid(4, [0, 0, 200, 255]);
    let mut badge = solid(2, [0, 0, 0, 0]);
    badge.rgba[8..12].copy_from_slice(&[255, 0, 0, 255]);
    let out = stamp(base.clone(), &badge);
    assert_eq!((out.width, out.height), (4, 4));
    let px = |b: &Bitmap, x: u32, y: u32| b.rgba[((y * b.width + x) * 4) as usize..][..4].to_vec();
    assert_eq!(px(&out, 0, 3), [255, 0, 0, 255]);
    assert_eq!(px(&out, 3, 0), [0, 0, 200, 255]);
    assert_eq!(px(&out, 1, 1), [0, 0, 200, 255]);
    let clear = stamp(base.clone(), &solid(4, [0, 0, 0, 0]));
    assert_eq!(clear.rgba, base.rgba);
}

#[test]
fn a_shortcut_icon_carries_the_link_arrow_and_its_target_does_not() {
    let root = crate::test_temp().join(format!("flit-shell-arrow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut sh = ShellCom::new().unwrap();
    let exe = p(r"C:\Windows\notepad.exe");
    let out = sh.file_op(&FileOp::Link { items: vec![exe.clone()], dest: p(root.to_str().unwrap()) }, &flit_platform::Job::detached(flit_core::provider::Resolve::Skip));
    let lnk = out.items[0].1.clone();
    assert!(sh.overlay(&lnk, 32).is_some());
    assert!(sh.overlay(&exe, 32).is_none());
    let icon = |sh: &mut ShellCom, path: &FsPath| sh.icon(&IconRequest { key: 0, kind: IconKind::File, path: path.clone(), size: 32, gen: 0, row: 0 }).unwrap();
    assert_ne!(icon(&mut sh, &lnk).rgba, icon(&mut sh, &exe).rgba);
    let typed = sh.icon(&IconRequest { key: 0, kind: IconKind::Type, path: lnk.clone(), size: 32, gen: 0, row: 0 }).unwrap();
    assert_eq!(typed.width.max(typed.height), 32);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn only_a_shell_overlay_answer_with_an_index_in_range_counts() {
    use windows::Win32::Foundation::{E_FAIL, S_FALSE, S_OK};
    let pending = windows::core::HRESULT(0x8000000A_u32 as i32);
    assert_eq!(overlay_result(S_OK, 3), Some(3));
    assert_eq!(overlay_result(S_OK, 15), Some(15));
    for (hr, index) in [(S_FALSE, 0), (S_FALSE, 3), (S_OK, 0), (S_OK, 16), (S_OK, -1), (S_OK, 300), (pending, 2), (E_FAIL, 2)] {
        assert_eq!(overlay_result(hr, index), None, "{hr:?} {index}");
    }
}

#[test]
fn overlays_are_asked_only_on_fixed_local_drives() {
    let temp = p(crate::test_temp().join("flit-overlay-dir").to_str().unwrap());
    assert!(overlay_allowed(&temp), "{}", temp.display());
    for other in [r"\\server\share\dir", r"\\wsl.localhost\Ubuntu\home", "/tmp"] {
        assert!(!overlay_allowed(&p(other)), "{other}");
    }
    let mut sh = ShellCom::new().unwrap();
    let t = std::time::Instant::now();
    assert_eq!(sh.overlay_slot(&p(r"\\flitnosuchserver\share\file.txt")), None);
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "a network path is refused before the shell sees it");
}

#[test]
fn a_plain_temporary_file_has_no_overlay_and_a_shortcut_gets_its_arrow_slot() {
    let root = crate::test_temp().join(format!("flit-shell-overlay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("folder")).unwrap();
    std::fs::write(root.join("plain.txt"), b"x").unwrap();
    let dir = p(root.to_str().unwrap());
    let mut sh = ShellCom::new().unwrap();
    assert_eq!(sh.overlay_slot(&dir.join(b"plain.txt")), None);
    assert_eq!(sh.overlay_slot(&dir.join(b"folder")), None);
    assert_eq!(sh.overlay_slot(&dir.join(b"missing.txt")), None, "a vanished file is not an error");
    let out = sh.file_op(&FileOp::Link { items: vec![p(r"C:\Windows\notepad.exe")], dest: dir.clone() }, &flit_platform::Job::detached(flit_core::provider::Resolve::Skip));
    let slot = sh.overlay_slot(&out.items[0].1).expect("the shell marks a shortcut");
    for size in [16, 32, 48] {
        let image = sh.overlay_image(slot, size).unwrap();
        assert!(image.width <= size && image.height <= size && image.rgba.iter().skip(3).step_by(4).any(|&a| a != 0), "{size}");
    }
    assert!(sh.overlay_image(0, 16).is_none() || sh.overlay_image(99, 16).is_none());
    assert_eq!(sh.overlay_slot(&dir.join(b"plain.txt")), None, "the folder kept for the next row answers the same");
    let _ = std::fs::remove_dir_all(&root);
}
