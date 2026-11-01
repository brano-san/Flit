use super::*;
use std::path::PathBuf;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

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
    ShellMenu { dir: t.path(""), items: vec![t.path("a.txt")], x: 0.0, y: 0.0, extended: false, menu, inline, shell: true, kind: flit_core::ui::MenuKind::None }
}

#[test]
fn shell_labels_lose_their_accelerators_and_split_off_their_keys() {
    assert_eq!(plain_label("Open &with	Ctrl+O"), ("Open with".to_owned(), "Ctrl+O".to_owned()));
    assert_eq!(plain_label("R&&D"), ("R&D".to_owned(), String::new()));
}

fn count(items: &[MenuItem]) -> usize {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Command { .. } => 1,
            MenuItem::Submenu { items, .. } => count(items),
            _ => 0,
        })
        .sum()
}

#[test]
fn shell_items_carry_one_shell_id_per_command() {
    crate::com_sta();
    let t = TempDir::new("own");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    let m = flit_menu(&t, false);
    unsafe {
        let hmenu = CreatePopupMenu().unwrap();
        let active = fill(HWND::default(), &m, hmenu).unwrap();
        let (items, ids) = shell_items(active.menu.as_ref(), hmenu);
        assert!(!matches!(items.first(), None | Some(MenuItem::Separator)), "{items:?}");
        assert_eq!(count(&items), ids.len());
        assert!(ids.iter().all(|&id| id == 0 || (FIRST_ID..=LAST_ID).contains(&id)), "{ids:?}");
        assert!(matches!(items.last(), Some(MenuItem::Submenu { label, .. }) if label == "Show more options"), "{items:?}");
        let _ = DestroyMenu(hmenu);
    }
}

fn wait_done(owner: isize) -> Vec<(Option<u64>, Event)> {
    let started = Instant::now();
    loop {
        let mut done = DONE.lock().unwrap();
        let (mine, rest): (Vec<_>, Vec<_>) = done.drain(..).partition(|(h, _, _)| *h == owner);
        *done = rest;
        drop(done);
        if !mine.is_empty() || started.elapsed() > Duration::from_secs(20) {
            return mine.into_iter().map(|(_, g, e)| (g, e)).collect();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_ready() -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        if READY.lock().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

fn target(t: &TempDir, items: &[&str]) -> ShellMenu {
    ShellMenu { items: items.iter().map(|n| t.path(n)).collect(), menu: vec![command("Open", "", false)], ..flit_menu(t, false) }
}

#[test]
fn a_prepared_menu_opens_complete_and_a_different_target_takes_the_usual_path() {
    let _serial = serial();
    let t = TempDir::new("prepared");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    std::fs::write(t.0.join("b.txt"), "x").unwrap();
    let owner = HWND::default();
    prefetch(owner, Some(target(&t, &["b.txt"])));
    prefetch(owner, Some(target(&t, &["a.txt"])));
    assert!(wait_ready(), "the latest target is prepared");
    assert!(READY.lock().unwrap().as_ref().is_some_and(|r| r.menu.same_target(&target(&t, &["a.txt"]))), "the superseded target is not");
    let seen = std::cell::RefCell::new(Vec::new());
    show(owner, &ShellMenu { x: 5.0, y: 7.0, ..target(&t, &["a.txt"]) }, &|ev| seen.borrow_mut().push(ev));
    let seen = seen.take();
    assert!(matches!(&seen[..], [Event::ShowMenu { pending: true, .. }, Event::ShellItems { items, shell }] if !shell.is_empty() && count(items) == shell.len()), "{seen:?}");
    assert!(READY.lock().unwrap().is_none() && WANT.lock().unwrap().is_none());
    pick_now(owner, None);
    let seen = std::cell::RefCell::new(Vec::new());
    show(owner, &target(&t, &["b.txt"]), &|ev| seen.borrow_mut().push(ev));
    assert!(matches!(&seen.borrow()[..], [Event::ShowMenu { pending: true, .. }]), "no prepared menu, so the shell items come later");
    let done = wait_done(0);
    assert!(matches!(&done[..], [(Some(_), Event::ShellItems { shell, .. })] if !shell.is_empty()), "{done:?}");
    pick_now(owner, None);
    prefetch(owner, Some(target(&t, &["b.txt"])));
    prefetch(owner, None);
    std::thread::sleep(PREP_REST * 3);
    assert!(READY.lock().unwrap().is_none(), "a cancelled target leaves nothing prepared");
}

#[test]
fn the_worker_thread_answers_only_the_latest_menu() {
    let _serial = serial();
    let t = TempDir::new("worker");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    let owner = HWND::default();
    let ask = |m: ShellMenu| {
        let gen = CURRENT.fetch_add(1, Ordering::SeqCst) + 1;
        send_job(Job::Menu { owner: 0, gen, menu: m, pt: POINT::default() });
        gen
    };
    let stale = ask(flit_menu(&t, true));
    let latest = ask(flit_menu(&t, false));
    let done = wait_done(0);
    assert_ne!(stale, latest);
    let [(gen, ev)] = &done[..] else { panic!("only the latest menu is answered: {done:?}") };
    assert_eq!(*gen, Some(latest));
    let Event::ShellItems { items, shell } = ev else { panic!("{ev:?}") };
    assert_eq!(count(items), shell.len());
    assert!(matches!(items.last(), Some(MenuItem::Submenu { label, .. }) if label == "Show more options"), "{items:?}");
    let seen = std::cell::RefCell::new(Vec::new());
    let send = |ev: Event| seen.borrow_mut().push(ev);
    run(owner, ShellUi::Done(Some(latest), Event::Timer), &send);
    pick_now(owner, None);
    run(owner, ShellUi::Done(Some(latest), Event::Timer), &send);
    run(owner, ShellUi::Done(None, Event::ShellVerb("copy".into())), &send);
    assert_eq!(*seen.borrow(), [Event::Timer, Event::ShellVerb("copy".into())], "answers for a closed menu are dropped");
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
fn program_items_come_first_then_the_rest_of_the_shell_in_a_submenu() {
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
            assert_ne!(all.first().map(String::as_str), Some("-"), "{all:?}");
            assert_eq!(all.last().map(String::as_str), Some("Show more options"));
            assert!(!all.windows(2).any(|w| w[0] == "-" && w[1] == "-"), "{all:?}");
            let count = GetMenuItemCount(Some(hmenu));
            for i in 0..count - 1 {
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
fn inline_shell_items_come_without_duplicates() {
    crate::com_sta();
    let t = TempDir::new("inline");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    unsafe {
        let hmenu = CreatePopupMenu().unwrap();
        let menu = fill(HWND::default(), &flit_menu(&t, true), hmenu).unwrap().menu.expect("inline menus query the shell");
        let all = texts(hmenu);
        assert!(!all.is_empty() && all[0] != "-" && all.last().unwrap() != "-" && !all.windows(2).any(|w| w[0] == "-" && w[1] == "-"), "{all:?}");
        let verbs = shell_verbs(&menu, hmenu);
        assert!(!verbs.is_empty() && !verbs.iter().any(|v| COVERED.contains(&v.as_str())), "{verbs:?}");
        let _ = DestroyMenu(hmenu);
    }
}

#[test]
fn a_command_picked_at_once_from_a_prepared_menu_reaches_the_worker_that_owns_it() {
    let _serial = serial();
    crate::com_sta();
    let t = TempDir::new("invoke");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    let m = target(&t, &["a.txt"]);
    let id = unsafe {
        let (menu, hmenu) = query(HWND::default(), &m).unwrap();
        let id = (FIRST_ID..LAST_ID).find(|&id| verb(&menu, (id - FIRST_ID) as usize).is_some_and(|v| v.eq_ignore_ascii_case("copy")));
        let _ = DestroyMenu(hmenu);
        id.expect("Copy is offered for a text file")
    };
    let owner = HWND::default();
    prefetch(owner, Some(m.clone()));
    assert!(wait_ready());
    show(owner, &m, &|_| {});
    pick_now(owner, Some(id));
    let done = wait_done(0);
    assert!(matches!(&done[..], [(None, Event::ShellVerb(v))] if v == "copy"), "{done:?}");
}

#[test]
#[ignore]
fn bench_right_click_to_complete_menu() {
    let _serial = serial();
    let kind = std::env::var("FLIT_MENU_BENCH").unwrap_or_else(|_| "txt".into());
    let t = TempDir::new(&format!("click-{kind}"));
    for k in 0..8 {
        std::fs::write(t.0.join(format!("f{k}.txt")), "x").unwrap();
        std::fs::write(t.0.join(format!("p{k}.png")), "x").unwrap();
        std::fs::create_dir(t.0.join(format!("d{k}"))).unwrap();
    }
    let owner = HWND::default();
    let menu = |k: usize| match kind.as_str() {
        "folder" => target(&t, &[&format!("d{k}")]),
        "background" => target(&t, &[]),
        "multi" => target(&t, &[&format!("f{k}.txt"), &format!("p{k}.png")]),
        _ => target(&t, &[&format!("f{k}.txt")]),
    };
    let click = |m: &ShellMenu| {
        let started = Instant::now();
        let complete = std::cell::Cell::new(false);
        show(owner, m, &|ev| complete.set(complete.get() || matches!(ev, Event::ShellItems { .. })));
        if !complete.get() {
            wait_done(0);
        }
        let ms = started.elapsed().as_secs_f64() * 1e3;
        pick_now(owner, None);
        std::thread::sleep(Duration::from_millis(30));
        ms
    };
    let warmed = std::env::var("FLIT_MENU_BENCH_WARM").is_ok();
    if warmed {
        warm_up(vec![target(&t, &["d7"]), target(&t, &["f7.txt"]), target(&t, &[])]);
        std::thread::sleep(Duration::from_secs(3));
    }
    let cold = click(&menu(0));
    let mut warm: Vec<f64> = (1..6).map(|k| click(&menu(k))).collect();
    warm.sort_by(f64::total_cmp);
    let mut prepared = Vec::new();
    let mut ready = Vec::new();
    for k in 1..6 {
        let m = menu(k);
        let started = Instant::now();
        prefetch(owner, Some(m.clone()));
        assert!(wait_ready());
        ready.push(started.elapsed().as_secs_f64() * 1e3);
        prepared.push(click(&m));
    }
    prepared.sort_by(f64::total_cmp);
    ready.sort_by(f64::total_cmp);
    let first = if warmed { "after warm-up" } else { "cold" };
    eprintln!("BENCH {kind:<10} {first:<13} first click {cold:>7.1} ms   warm median {:>6.1} ms (max {:>6.1})   prefetched median {:>6.3} ms (max {:>6.3})   prepare after rest {:>6.1} ms", warm[2], warm[4], prepared[2], prepared[4], ready[2]);
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
    let shell_menu = |k: usize, inline: bool| ShellMenu { dir: t.path(""), items: vec![t.path(&format!("f{k}.txt"))], x: 0.0, y: 0.0, extended: false, menu: flit.clone(), inline, shell: true, kind: flit_core::ui::MenuKind::None };
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
    let items = verbs(&ShellMenu { dir: t.path(""), items: vec![t.path("a.txt")], x: 0.0, y: 0.0, extended: false, menu: Vec::new(), inline: false, shell: true, kind: flit_core::ui::MenuKind::None });
    for v in ["copy", "cut", "delete", "rename", "properties"] {
        assert!(items.iter().any(|i| i == v), "{v} missing from {items:?}");
    }
    let background = verbs(&ShellMenu { dir: t.path(""), items: Vec::new(), x: 0.0, y: 0.0, extended: false, menu: Vec::new(), inline: false, shell: true, kind: flit_core::ui::MenuKind::None });
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
    let m = ShellMenu { dir, items, x: 0.0, y: 0.0, extended: false, menu: vec![command("(Flit items)", "", false)], inline, shell: true, kind: flit_core::ui::MenuKind::None };
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
