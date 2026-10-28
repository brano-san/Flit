use super::*;
use std::path::PathBuf;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = crate::test_temp().join(format!("flit-menu-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, rel: &str) -> FsPath {
        FsPath::parse(self.0.join(rel).to_str().unwrap()).unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

unsafe fn shell_verbs(menu: &IContextMenu, hmenu: HMENU) -> Vec<String> {
    let ids: Vec<u32> = (0..GetMenuItemCount(Some(hmenu))).map(|i| GetMenuItemID(hmenu, i)).filter(|id| (FIRST_ID..=LAST_ID).contains(id)).collect();
    ids.iter().filter_map(|&id| verb(menu, (id - FIRST_ID) as usize)).map(|v| v.to_ascii_lowercase()).collect()
}

fn verbs(m: &ShellMenu) -> Vec<String> {
    crate::com_sta();
    unsafe {
        let menu = build(HWND::default(), m).unwrap();
        let hmenu = CreatePopupMenu().unwrap();
        menu.QueryContextMenu(hmenu, 0, FIRST_ID, LAST_ID, CMF_NORMAL | CMF_CANRENAME).ok().unwrap();
        let out = shell_verbs(&menu, hmenu);
        let _ = DestroyMenu(hmenu);
        out
    }
}

unsafe fn texts(hmenu: HMENU) -> Vec<String> {
    (0..GetMenuItemCount(Some(hmenu)))
        .map(|i| {
            if is_separator(hmenu, i) {
                return "-".to_owned();
            }
            let mut buf = [0u16; 128];
            let n = GetMenuStringW(hmenu, i as u32, Some(&mut buf), MF_BYPOSITION) as usize;
            String::from_utf16_lossy(&buf[..n])
        })
        .collect()
}

fn command(label: &str, keys: &str, checked: bool) -> MenuItem {
    MenuItem::command(label, keys).checked(checked)
}

fn flit_menu(t: &TempDir, inline: bool) -> ShellMenu {
    let menu = vec![command("Open", "Enter", false), MenuItem::Separator, MenuItem::Submenu { label: "Sort by".into(), items: vec![command("Name", "", true)] }, command("R&D", "", false)];
    ShellMenu { dir: t.path(""), items: vec![t.path("a.txt")], x: 0.0, y: 0.0, extended: false, menu, inline, shell: true }
}

#[test]
fn toggles_are_found_by_the_same_ids_the_native_menu_gets() {
    let toggle = |label: &str| MenuItem::command(label, String::new()).checked(true).staying();
    let menu = vec![command("Collapse", "", false), MenuItem::Separator, MenuItem::Submenu { label: "Sections".into(), items: vec![toggle("Pinned"), toggle("Drives")] }, toggle("Compact")];
    assert_eq!(toggle_ids(&menu), [FLIT_FIRST + 1, FLIT_FIRST + 2, FLIT_FIRST + 3]);
    assert!(toggle_ids(&flit_menu(&TempDir::new("toggles"), false).menu).is_empty());
}

fn fake(verb: &str) -> ShellItem<'_> {
    ShellItem { verb, usable: true, children: None }
}

#[test]
fn shell_items_from_programs_are_kept_and_duplicates_of_flit_and_windows_items_are_not() {
    let seven_zip = ["SevenZipCompress".to_owned(), "SevenZip.Checksum".to_owned(), "SevenZip.Checksum.Calc.CRC32".to_owned()];
    let send_to = [String::new(), String::new()];
    let library = ["PinToStartScreen".to_owned()];
    let items = [
        ("Open", fake("open")),
        ("Copilot", fake("{DB6BACB1-FC31-4B28-9C53-7240A4F98404}")),
        ("Edit", fake("edit")),
        ("Edit with Notepad++", fake("{E6950302-61F0-4FEB-97DB-855E30D4A991}")),
        ("Edit with Notepad++ (legacy)", ShellItem { verb: "ANotepad++64", usable: false, children: None }),
        ("Add to Favorites", fake("pintohomefile")),
        ("Open with Code", fake("VSCode")),
        ("Git Bash here", fake("git_shell")),
        ("7-Zip", ShellItem { verb: "SevenZip", usable: true, children: Some(&seven_zip) }),
        ("What's locking this file?", fake("")),
        ("Open with", ShellItem { verb: "openas", usable: true, children: Some(&[]) }),
        ("Give access to", ShellItem { verb: "Windows.Share", usable: true, children: Some(&send_to) }),
        ("Copy as path", fake("copyaspath")),
        ("Share", fake("Windows.ModernShare")),
        ("Restore previous versions", fake("PreviousVersions")),
        ("Include in library", ShellItem { verb: "", usable: true, children: Some(&library) }),
        ("Send to", ShellItem { verb: "", usable: true, children: Some(&send_to) }),
        ("Cut", fake("cut")),
        ("Copy", fake("COPY")),
        ("Create shortcut", fake("link")),
        ("Delete", fake("delete")),
        ("Rename", fake("rename")),
        ("Properties", fake("properties")),
        ("Open in new tab", fake("opennewtab")),
        ("blank", ShellItem { verb: "", usable: false, children: None }),
    ];
    let by = |want: Place| items.iter().filter(|(_, i)| place(i) == want).map(|(l, _)| *l).collect::<Vec<_>>();
    assert_eq!(by(Place::App), ["Copilot", "Edit with Notepad++", "Open with Code", "Git Bash here", "7-Zip", "What's locking this file?"]);
    assert_eq!(by(Place::Duplicate), ["Open", "Copy as path", "Cut", "Copy", "Create shortcut", "Delete", "Rename", "Properties", "Open in new tab"]);
    assert_eq!(
        by(Place::Windows),
        ["Edit", "Edit with Notepad++ (legacy)", "Add to Favorites", "Open with", "Give access to", "Share", "Restore previous versions", "Include in library", "Send to", "blank"]
    );
}

#[test]
fn flit_items_come_first_then_program_items_then_the_rest_of_the_shell_in_a_submenu() {
    crate::com_sta();
    let t = TempDir::new("lifted");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    std::fs::create_dir(t.0.join("dir")).unwrap();
    let single = flit_menu(&t, false);
    let several = ShellMenu { items: vec![t.path("a.txt"), t.path("dir")], ..flit_menu(&t, false) };
    for m in [single, several] {
        unsafe {
            let hmenu = CreatePopupMenu().unwrap();
            let menu = fill(HWND::default(), &m, hmenu).unwrap().menu.expect("the shell menu is asked before the menu shows");
            let all = texts(hmenu);
            assert_eq!(all[..5], ["Open\tEnter", "-", "Sort by", "R&&D", "-"], "{all:?}");
            assert_eq!(all.last().map(String::as_str), Some("Show more options"));
            assert!(!all.windows(2).any(|w| w[0] == "-" && w[1] == "-"), "{all:?}");
            let sort = GetSubMenu(hmenu, 2);
            assert_eq!((GetMenuItemID(hmenu, 0), GetMenuItemID(sort, 0), GetMenuItemID(hmenu, 3)), (FLIT_FIRST, FLIT_FIRST + 1, FLIT_FIRST + 2));
            assert_ne!(GetMenuState(sort, 0, MF_BYPOSITION) & MF_CHECKED.0, 0);
            let count = GetMenuItemCount(Some(hmenu));
            for i in 5..count - 1 {
                if is_separator(hmenu, i) {
                    continue;
                }
                assert_eq!(place_at(&menu, hmenu, i), Some(Place::App), "{} is not a program's item", all[i as usize]);
                let mut info = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_ID | MIIM_SUBMENU, ..Default::default() };
                GetMenuItemInfoW(hmenu, i as u32, true, &mut info).unwrap();
                assert!(!info.hSubMenu.is_invalid() || (FIRST_ID..=LAST_ID).contains(&info.wID), "{} keeps the shell's command id", all[i as usize]);
            }
            let more = GetSubMenu(hmenu, count - 1);
            let rest: Vec<Option<Place>> = (0..GetMenuItemCount(Some(more))).map(|i| place_at(&menu, more, i)).collect();
            assert!(!rest.contains(&Some(Place::App)), "{:?}", texts(more));
            let verbs = shell_verbs(&menu, more);
            assert!(["copy", "delete", "properties"].iter().all(|v| verbs.iter().any(|x| x == v)), "{verbs:?}");
            let _ = DestroyMenu(hmenu);
        }
    }
}

#[test]
fn inline_shell_items_follow_flit_items_without_duplicates() {
    crate::com_sta();
    let t = TempDir::new("inline");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    unsafe {
        let hmenu = CreatePopupMenu().unwrap();
        let menu = fill(HWND::default(), &flit_menu(&t, true), hmenu).unwrap().menu.expect("inline menus query the shell up front");
        let all = texts(hmenu);
        assert_eq!(all[..5], ["Open\tEnter", "-", "Sort by", "R&&D", "-"]);
        assert!(all.len() > 5 && all.last().unwrap() != "-" && !all.windows(2).any(|w| w[0] == "-" && w[1] == "-"), "{all:?}");
        let verbs = shell_verbs(&menu, hmenu);
        assert!(!verbs.is_empty() && !verbs.iter().any(|v| COVERED.contains(&v.as_str())), "{verbs:?}");
        let _ = DestroyMenu(hmenu);
    }
}

#[test]
#[ignore]
fn bench_menu_prepare() {
    crate::com_sta();
    let t = TempDir::new("bench");
    let runs = 15;
    for k in 0..runs {
        std::fs::write(t.0.join(format!("f{k}.txt")), "x").unwrap();
    }
    let flit: Vec<MenuItem> = (0..16).map(|k| if k % 5 == 4 { MenuItem::Separator } else { command(&format!("Item {k}"), "Ctrl+Shift+C", false) }).collect();
    let shell_menu = |k: usize, inline: bool| ShellMenu { dir: t.path(""), items: vec![t.path(&format!("f{k}.txt"))], x: 0.0, y: 0.0, extended: false, menu: flit.clone(), inline, shell: true };
    let report = |label: &str, mut ms: Vec<f64>| {
        let first = ms[0];
        ms.sort_by(f64::total_cmp);
        eprintln!("BENCH {label:<44} first {first:>9.3} ms   median {:>9.3} ms   max {:>9.3} ms", ms[ms.len() / 2], ms[ms.len() - 1]);
    };
    let time = |f: &dyn Fn(usize)| (0..runs).map(|k| {
        let started = Instant::now();
        f(k);
        started.elapsed().as_secs_f64() * 1e3
    }).collect::<Vec<_>>();
    let shell_only = time(&|k| unsafe {
        let (_, h) = query(HWND::default(), &shell_menu(k, false)).unwrap();
        let _ = DestroyMenu(h);
    });
    let lazy = time(&|k| unsafe {
        let h = CreatePopupMenu().unwrap();
        drop(fill(HWND::default(), &shell_menu(k, false), h).unwrap());
        let _ = DestroyMenu(h);
    });
    let inline = time(&|k| unsafe {
        let h = CreatePopupMenu().unwrap();
        drop(fill(HWND::default(), &shell_menu(k, true), h).unwrap());
        let _ = DestroyMenu(h);
    });
    report("menu before show: shell items (old)", shell_only);
    report("menu before show: Flit, program items, submenu", lazy);
    report("menu before show: inline shell items", inline);
}

#[test]
fn item_and_background_menus_offer_the_verbs_flit_takes_over() {
    let t = TempDir::new("verbs");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    let items = verbs(&ShellMenu { dir: t.path(""), items: vec![t.path("a.txt")], x: 0.0, y: 0.0, extended: false, menu: Vec::new(), inline: false, shell: true });
    for v in ["copy", "cut", "delete", "rename", "properties"] {
        assert!(items.iter().any(|i| i == v), "{v} missing from {items:?}");
    }
    let background = verbs(&ShellMenu { dir: t.path(""), items: Vec::new(), x: 0.0, y: 0.0, extended: false, menu: Vec::new(), inline: false, shell: true });
    assert!(background.iter().any(|v| v == "properties"), "{background:?}");
    assert!(SHELL_VERBS.iter().all(|v| v.chars().all(|c| c.is_ascii_lowercase())));
}

#[test]
fn shell_data_objects_carry_the_dragged_paths() {
    crate::com_sta();
    let t = TempDir::new("data");
    for n in ["one.txt", "two words.txt"] {
        std::fs::write(t.0.join(n), n).unwrap();
    }
    let paths = vec![t.path("one.txt"), t.path("two words.txt")];
    let data: IDataObject = Pidls::parse(&paths).unwrap().ui_object(HWND::default()).unwrap();
    assert_eq!(crate::dragdrop::data_paths(&data), paths);
    assert!(Pidls::parse(&[t.path("missing.txt")]).is_err());
}

struct Recycled(Vec<(FsPath, FsPath)>);

impl Drop for Recycled {
    fn drop(&mut self) {
        let left: Vec<(FsPath, FsPath)> = self.0.iter().filter(|(r, _)| crate::fileops::std_path(r).exists()).cloned().collect();
        let _ = crate::recycle::purge(&left);
    }
}

unsafe fn shell_menu_of(paths: &[FsPath]) -> (Vec<String>, Vec<String>) {
    let menu: IContextMenu = Pidls::parse(paths).unwrap().ui_object(HWND::default()).unwrap();
    let hmenu = CreatePopupMenu().unwrap();
    menu.QueryContextMenu(hmenu, 0, FIRST_ID, LAST_ID, CMF_NORMAL).ok().unwrap();
    let out = (texts(hmenu), shell_verbs(&menu, hmenu));
    let _ = DestroyMenu(hmenu);
    out
}

#[test]
#[ignore]
fn recycled_items_get_the_bins_own_menu_and_restore_or_purge_only_themselves() {
    use flit_core::provider::{FileOp, Resolve};
    use flit_platform::{FileOps, Job, Shell};
    crate::com_sta();
    let t = TempDir::new("bin");
    let originals: Vec<FsPath> = ["one.txt", "two.txt", "three.txt"]
        .iter()
        .map(|n| {
            std::fs::write(t.0.join(n), n).unwrap();
            t.path(n)
        })
        .collect();
    let (done, left) = crate::recycle::trash(&originals);
    assert!(left.is_empty(), "{left:?}");
    let guard = Recycled(done.into_iter().map(|(original, recycled)| (recycled, original)).collect());
    let recycled: Vec<FsPath> = guard.0.iter().map(|(r, _)| r.clone()).collect();
    for items in [&recycled[..1], &recycled[..]] {
        let (labels, verbs) = unsafe { shell_menu_of(items) };
        eprintln!("BIN ITEMS x{}: {labels:?} {verbs:?}", items.len());
        for v in ["undelete", "cut", "delete", "properties"] {
            assert!(verbs.iter().any(|x| x == v), "{v} missing from {verbs:?}");
        }
        assert!(verbs.iter().all(|v| v != "copy" && v != "rename"), "{verbs:?}");
    }
    let (labels, verbs) = unsafe { shell_menu_of(&[FsPath::recycle_bin()]) };
    eprintln!("BIN ITSELF: {labels:?} {verbs:?}");
    assert!(verbs.iter().any(|v| v == "properties"), "{verbs:?}");
    let mut sh = crate::ShellCom::new().unwrap();
    let job = Job::detached(Resolve::KeepBoth);
    let out = sh.file_op(&FileOp::Restore(guard.0[..1].to_vec()), &job);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert!(std::fs::read(t.0.join("one.txt")).is_ok_and(|b| b == b"one.txt") && !crate::fileops::std_path(&recycled[0]).exists());
    let out = sh.file_op(&FileOp::Purge(guard.0[1..].to_vec()), &job);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.items.len(), 2);
    assert!(recycled.iter().all(|r| !crate::fileops::std_path(r).exists()));
    assert!(Pidls::parse(&recycled[1..]).is_err(), "purged items are gone from the bin");
}

unsafe fn walk_labels(menu: &IContextMenu, hmenu: HMENU, depth: usize, out: &mut Vec<String>) {
    if let Ok(m3) = menu.cast::<IContextMenu3>() {
        let _ = m3.HandleMenuMsg2(WM_INITMENUPOPUP, WPARAM(hmenu.0 as usize), LPARAM(0), None);
    } else if let Ok(m2) = menu.cast::<IContextMenu2>() {
        let _ = m2.HandleMenuMsg(WM_INITMENUPOPUP, WPARAM(hmenu.0 as usize), LPARAM(0));
    }
    for i in 0..GetMenuItemCount(Some(hmenu)) {
        let mut buf = [0u16; 256];
        let mut info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STRING | MIIM_SUBMENU | MIIM_ID | MIIM_FTYPE | MIIM_BITMAP | MIIM_STATE,
            dwTypeData: windows::core::PWSTR(buf.as_mut_ptr()),
            cch: buf.len() as u32,
            ..Default::default()
        };
        let _ = GetMenuItemInfoW(hmenu, i as u32, true, &mut info);
        if info.fType.0 & MFT_SEPARATOR.0 != 0 {
            out.push(format!("{}-", "  ".repeat(depth)));
            continue;
        }
        let label = String::from_utf16_lossy(&buf[..info.cch as usize]);
        let verb = (FIRST_ID..=LAST_ID).contains(&info.wID).then(|| verb(menu, (info.wID - FIRST_ID) as usize)).flatten().unwrap_or_default();
        let icon = if info.hbmpItem.is_invalid() { "" } else { " [icon]" };
        let grey = if info.fState.0 & MFS_DISABLED.0 != 0 { " [disabled]" } else { "" };
        out.push(format!("{}{label} <{verb}> #{}{icon}{grey}", "  ".repeat(depth), info.wID));
        if !info.hSubMenu.is_invalid() {
            walk_labels(menu, info.hSubMenu, depth + 1, out);
        }
    }
}

#[test]
#[ignore]
fn print_the_shell_menu_of_flit_menu_probe() {
    crate::com_sta();
    let probe = std::env::var("FLIT_MENU_PROBE").expect("FLIT_MENU_PROBE=<path>[|<path>…]");
    let items: Vec<FsPath> = probe.split('|').filter_map(FsPath::parse).collect();
    let dir = items[0].parent().unwrap();
    let inline = std::env::var("FLIT_MENU_INLINE").is_ok();
    let m = ShellMenu { dir, items, x: 0.0, y: 0.0, extended: false, menu: vec![command("(Flit items)", "", false)], inline, shell: true };
    unsafe {
        let hmenu = CreatePopupMenu().unwrap();
        let started = Instant::now();
        let menu = fill(HWND::default(), &m, hmenu).unwrap().menu.unwrap();
        let ms = started.elapsed().as_secs_f64() * 1e3;
        let mut out = Vec::new();
        walk_labels(&menu, hmenu, 0, &mut out);
        eprintln!("PROBE {ms:.1} ms\n{}", out.join("\n"));
        let _ = DestroyMenu(hmenu);
    }
}
