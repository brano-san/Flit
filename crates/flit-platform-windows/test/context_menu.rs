use super::*;
use std::path::PathBuf;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("flit-menu-{tag}-{}", std::process::id()));
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
    MenuItem::Command { label: label.into(), keys: keys.into(), checked }
}

fn flit_menu(t: &TempDir, inline: bool) -> ShellMenu {
    let menu = vec![command("Open", "Enter", false), MenuItem::Separator, MenuItem::Submenu { label: "Sort by".into(), items: vec![command("Name", "", true)] }, command("R&D", "", false)];
    ShellMenu { dir: t.path(""), items: vec![t.path("a.txt")], x: 0.0, y: 0.0, extended: false, menu, inline }
}

#[test]
fn flit_items_come_first_and_shell_items_wait_for_their_submenu() {
    crate::com_sta();
    let t = TempDir::new("lazy");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    unsafe {
        let hmenu = CreatePopupMenu().unwrap();
        let active = fill(HWND::default(), &flit_menu(&t, false), hmenu).unwrap();
        assert!(active.menu.is_none());
        assert_eq!(texts(hmenu), ["Open\tEnter", "-", "Sort by", "R&&D", "-", "Show more options"]);
        let sort = GetSubMenu(hmenu, 2);
        assert_eq!((GetMenuItemID(hmenu, 0), GetMenuItemID(sort, 0), GetMenuItemID(hmenu, 3)), (FLIT_FIRST, FLIT_FIRST + 1, FLIT_FIRST + 2));
        assert_ne!(GetMenuState(sort, 0, MF_BYPOSITION) & MF_CHECKED.0, 0);
        let more = GetSubMenu(hmenu, 5);
        assert_eq!(GetMenuItemCount(Some(more)), 0);
        ACTIVE.set(Some(active));
        assert_eq!(forward(WM_INITMENUPOPUP, WPARAM(more.0 as usize), LPARAM(5)), Some(LRESULT(0)));
        let filled = ACTIVE.take().and_then(|a| a.menu).expect("the shell menu stays for its command");
        let verbs = shell_verbs(&filled, more);
        assert!(["copy", "delete", "properties"].iter().all(|v| verbs.iter().any(|x| x == v)), "{verbs:?}");
        let _ = DestroyMenu(hmenu);
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
    let shell_menu = |k: usize, inline: bool| ShellMenu { dir: t.path(""), items: vec![t.path(&format!("f{k}.txt"))], x: 0.0, y: 0.0, extended: false, menu: flit.clone(), inline };
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
    report("menu before show: Flit items + submenu (new)", lazy);
    report("menu before show: inline shell items", inline);
}

#[test]
fn item_and_background_menus_offer_the_verbs_flit_takes_over() {
    let t = TempDir::new("verbs");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    let items = verbs(&ShellMenu { dir: t.path(""), items: vec![t.path("a.txt")], x: 0.0, y: 0.0, extended: false, menu: Vec::new(), inline: false });
    for v in ["copy", "cut", "delete", "rename", "properties"] {
        assert!(items.iter().any(|i| i == v), "{v} missing from {items:?}");
    }
    let background = verbs(&ShellMenu { dir: t.path(""), items: Vec::new(), x: 0.0, y: 0.0, extended: false, menu: Vec::new(), inline: false });
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
