use super::*;
use crate::listing::{ChangeKind, DirChange, DirSnapshot, Listing, Meta};
use crate::provider::ProviderEvent;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::ui::{Drive, MenuItem, Persisted};
use std::sync::Arc;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };
const ALT: Mods = Mods { ctrl: false, shift: false, alt: true };
const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

fn t0() -> Instant {
    Instant::now()
}

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn app_at(path: &str) -> App {
    let state = Persisted { path: Location::parse(path).unwrap(), ..Default::default() };
    App::new(state, Vec::new(), 0, false, 1.0, (1000, 600), t0())
}

fn deliver(app: &mut App, items: &[(&str, bool)]) {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, size: 1, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
}

fn key(app: &mut App, key: Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, t0());
}

fn typed(app: &mut App, s: &str) {
    s.chars().for_each(|c| app.on_event(&Event::Char(c), t0()));
}

fn row_xy(app: &App, row: usize) -> (f32, f32) {
    let l = &app.layout;
    (l.cols[0].x + 100.0, l.list.y + (row as f32 + 0.5) * l.row_h)
}

fn press(app: &mut App, (x, y): (f32, f32), button: MouseButton, mods: Mods) {
    app.on_event(&Event::Key { key: Key::Other, mods, pressed: false }, t0());
    app.on_event(&Event::MouseDown { x, y, button, clicks: 1 }, t0());
}

fn release(app: &mut App, (x, y): (f32, f32), button: MouseButton) {
    app.on_event(&Event::MouseUp { x, y, button }, t0());
}

fn select(app: &mut App, names: &[&str]) {
    let v = app.tabs[app.cur].view.as_mut().unwrap();
    v.clear_selection();
    for n in names {
        let i = v.find(n.as_bytes()).unwrap();
        v.set_selected(i, true);
        v.focus = Some(i);
    }
}

fn selected(app: &App) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.selected().map(|i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn ops(app: &mut App) -> Vec<(OpId, FileOp)> {
    app.drain_effects()
        .filter_map(|e| match e {
            Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Op { id, op, .. } } => Some((id, op)),
            _ => None,
        })
        .collect()
}

fn done(app: &mut App, id: OpId, op: FileOp, items: &[(&str, &str)]) {
    let tab = app.tab().id;
    let report = OpReport { op, items: items.iter().map(|&(a, b)| (p(a), p(b))).collect() };
    app.on_provider_event(ProviderEvent::OpDone { tab, id, error: None, report: Some(report) }, t0());
}

fn change(app: &mut App, kind: ChangeKind, name: &str, dir: bool) {
    let (tab, gen) = (app.tab().id, app.tab().gen);
    let meta = Some(Meta { attrs: if dir { attr::DIR } else { 0 }, ..Default::default() });
    app.on_provider_event(ProviderEvent::Changes { tab, gen, changes: vec![DirChange { kind, name: name.as_bytes().to_vec(), meta }] }, t0());
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
}

#[test]
fn names_are_validated_like_windows_does() {
    assert_eq!(validate_name("  report.txt  ", true), Ok("report.txt".into()));
    assert_eq!(validate_name("draft. . ", true), Ok("draft".into()));
    assert!(validate_name("a:b", true).is_err());
    assert!(validate_name("a|b", true).is_err());
    assert!(validate_name("tab\there", true).is_err());
    for reserved in ["CON", "con.txt", "Nul", "COM1", "lpt9.log", "COM¹", "aux .md"] {
        assert_eq!(validate_name(reserved, true), Err("This name is reserved by Windows"), "{reserved}");
    }
    for fine in ["CONSOLE", "COM10", "LPTX", "com", "nul-ish"] {
        assert!(validate_name(fine, true).is_ok(), "{fine}");
    }
    assert!(validate_name("   ", true).is_err());
    assert!(validate_name("..", true).is_err());
    assert!(validate_name(&"x".repeat(256), true).is_err());
    assert_eq!(validate_name("a:b", false), Ok("a:b".into()));
    assert!(validate_name("a/b", false).is_err());
}

#[test]
fn naming_helpers() {
    let taken = ["New folder", "New folder (2)"];
    assert_eq!(unique_name("New folder", |n| taken.contains(&n)), "New folder (3)");
    assert_eq!(unique_name("New folder", |_| false), "New folder");
    assert_eq!(base_name_len("photo.final.jpg", false), "photo.final".len());
    assert_eq!(base_name_len(".gitignore", false), ".gitignore".len());
    assert_eq!(base_name_len("archive.tar", true), "archive.tar".len());
    assert_eq!(base_name_len("README", false), 6);
}

#[test]
fn drop_effects_follow_explorer_rules() {
    use drop_effect::*;
    let all = COPY | MOVE | LINK;
    assert_eq!(choose_effect(all, NO, true), MOVE);
    assert_eq!(choose_effect(all, NO, false), COPY);
    assert_eq!(choose_effect(all, CTRL, true), COPY);
    assert_eq!(choose_effect(all, SHIFT, false), MOVE);
    assert_eq!(choose_effect(all, ALT, true), LINK);
    assert_eq!(choose_effect(all, CTRL_SHIFT, true), LINK);
    assert_eq!(choose_effect(COPY, NO, true), COPY);
    assert_eq!(choose_effect(COPY, SHIFT, true), NONE);
    assert!(same_volume(&p(r"C:\a"), &p(r"c:\b\c"), &[]));
    assert!(!same_volume(&p(r"C:\a"), &p(r"D:\a"), &[]));
    assert!(same_volume(&p(r"\\srv\share\a"), &p(r"\\SRV\Share\b"), &[]));
    assert!(!same_volume(&p(r"\\srv\one\a"), &p(r"\\srv\two\a"), &[]));
}

#[test]
fn unix_volumes_come_from_the_mount_list() {
    let drive = |path: &str| Drive { path: p(path), label: String::new(), total: 0, free: 0, file_system: String::new(), removable: false, unmounted: false, kind: None, remote: None };
    let mounts = [drive("/"), drive("/home"), drive("/media/u/USB"), drive(r"C:\")];
    assert!(same_volume(&p("/home/a"), &p("/home/b/c"), &mounts));
    assert!(!same_volume(&p("/home/a"), &p("/media/u/USB/x"), &mounts));
    assert!(!same_volume(&p("/etc"), &p("/home"), &mounts));
    assert!(same_volume(&p("/etc"), &p("/usr/lib"), &mounts));
    assert!(!same_volume(&p("/media/u/usb/x"), &p("/media/u/USB/x"), &mounts), "mount points are case-sensitive");
    assert!(same_volume(&p("/a"), &p("/b"), &[]), "without a mount list everything is one volume");
    assert!(!same_volume(&p("/a"), &p(r"C:\a"), &mounts));
}

fn drive_fs(path: &str, file_system: &str) -> Drive {
    Drive { path: p(path), label: String::new(), total: 0, free: 0, file_system: file_system.into(), removable: false, unmounted: false, kind: None, remote: None }
}

#[test]
fn unix_folders_on_windows_file_systems_take_windows_names() {
    let smb = "/run/user/1000/gvfs/smb-share:server=nas,share=docs";
    let mounts = [
        drive_fs("/", "ext4"),
        drive_fs("/media/u/STICK", "vfat"),
        drive_fs("/mnt/games", "ntfs3"),
        drive_fs("/mnt/old", "fuseblk"),
        Drive { unmounted: true, ..drive_fs("/dev/sdb1", "exfat") },
        Drive { remote: Some("smb-share:server=nas,share=docs".into()), ..drive_fs(smb, "") },
    ];
    assert!(windows_names(&p(r"C:\x"), &[]));
    assert!(windows_names(&p(r"\\srv\share\x"), &mounts));
    assert!(!windows_names(&p("/home/u"), &mounts));
    assert!(!windows_names(&p("/home/u"), &[]), "without a mount list Unix rules apply");
    assert!(windows_names(&p("/media/u/STICK"), &mounts));
    assert!(windows_names(&p("/media/u/STICK/photos/2024"), &mounts));
    assert!(!windows_names(&p("/media/u/stick/photos"), &mounts), "mount points are case-sensitive");
    assert!(windows_names(&p("/mnt/games/steam"), &mounts));
    assert!(windows_names(&p("/mnt/old"), &mounts));
    assert!(!windows_names(&p("/dev/sdb1"), &mounts), "an unmounted partition says nothing about its path");
    assert!(windows_names(&p(&format!("{smb}/reports")), &mounts));
    assert!(bad_name_char(':', true) && !bad_name_char(':', false));
    assert!(bad_name_char('/', false) && bad_name_char('\0', false) && !bad_name_char('\\', false));
}

#[test]
fn unix_names_allow_windows_characters_and_count_bytes() {
    assert_eq!(validate_name("a:b?.txt ", false), Ok("a:b?.txt".into()));
    assert_eq!(validate_name("CON", false), Ok("CON".into()));
    assert_eq!(validate_name("trailing.", false), Ok("trailing.".into()));
    let cyrillic = "я".repeat(128);
    assert!(validate_name(&cyrillic, true).is_ok(), "128 UTF-16 units fit on Windows");
    assert_eq!(validate_name(&cyrillic, false), Err("The name is too long"), "256 bytes do not fit on Linux");
    assert!(validate_name(&"x".repeat(255), false).is_ok());
}

#[test]
fn renaming_on_a_unix_path_uses_unix_name_rules() {
    let mut app = app_at("/home/u");
    app.caps = crate::caps::Caps { unix_paths: true };
    deliver(&mut app, &[("a.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::F(2), NO);
    key(&mut app, Key::Char('A'), CTRL);
    typed(&mut app, "x:y.txt");
    key(&mut app, Key::Enter, NO);
    let [(_, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Rename { item: p("/home/u/a.txt"), name: b"x:y.txt".to_vec() });
}

#[test]
fn copy_and_cut_put_the_selection_on_the_clipboard() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    key(&mut app, Key::Char('C'), CTRL);
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::SetClipboardFiles { .. })), "nothing selected");
    select(&mut app, &["a.txt", "b.txt"]);
    key(&mut app, Key::Char('C'), CTRL);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::SetClipboardFiles { paths, cut: false } if paths == [p(r"C:\x\a.txt"), p(r"C:\x\b.txt")])));
    key(&mut app, Key::Char('X'), CTRL);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::SetClipboardFiles { cut: true, .. })));
    key(&mut app, Key::Char('C'), CTRL_SHIFT);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::SetClipboard(_))), "Ctrl+Shift+C still copies paths as text");
}

#[test]
fn paste_selects_the_pasted_items_and_can_be_undone() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    key(&mut app, Key::Char('V'), CTRL);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Paste { dest: p(r"C:\x") });
    let copy = FileOp::Copy { items: vec![p(r"D:\in\b.txt"), p(r"D:\in\c.txt")], dest: p(r"C:\x") };
    done(&mut app, id, copy, &[(r"D:\in\b.txt", r"C:\x\b.txt"), (r"D:\in\c.txt", r"C:\x\c.txt")]);
    change(&mut app, ChangeKind::Added, "b.txt", false);
    assert_eq!(selected(&app), ["b.txt"]);
    change(&mut app, ChangeKind::Added, "c.txt", false);
    assert_eq!(selected(&app), ["b.txt", "c.txt"]);
    assert_eq!(app.undo_depth(), (1, 0));
    key(&mut app, Key::Char('Z'), CTRL);
    let [(undo_id, undo)] = ops(&mut app).try_into().unwrap();
    assert_eq!(undo, FileOp::Trash(vec![p(r"C:\x\b.txt"), p(r"C:\x\c.txt")]));
    done(&mut app, undo_id, undo, &[(r"C:\x\b.txt", r"C:\$Recycle.Bin\S\$R1"), (r"C:\x\c.txt", r"C:\$Recycle.Bin\S\$R2")]);
    assert_eq!(app.undo_depth(), (0, 1));
    key(&mut app, Key::Char('Y'), CTRL);
    let [(_, redo)] = ops(&mut app).try_into().unwrap();
    assert_eq!(redo, FileOp::Restore(vec![(p(r"C:\$Recycle.Bin\S\$R1"), p(r"C:\x\b.txt")), (p(r"C:\$Recycle.Bin\S\$R2"), p(r"C:\x\c.txt"))]));
    key(&mut app, Key::Char('Y'), CTRL);
    assert!(ops(&mut app).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some("Nothing to redo"));
}

#[test]
fn deleting_permanently_cannot_be_undone() {
    let mut app = app_at(r"\\srv\share\d");
    deliver(&mut app, &[("a.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Delete, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Trash(vec![p(r"\\srv\share\d\a.txt")]));
    done(&mut app, id, op, &[(r"\\srv\share\d\a.txt", r"\\srv\share\d\a.txt")]);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('Z'), CTRL);
    assert!(ops(&mut app).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some(crate::i18n::S::ErrorsOpsNotRecycled.en()));
}

#[test]
fn f2_renames_inline_and_keeps_the_item_focused() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("photo.jpg", false), ("z", false)]);
    select(&mut app, &["photo.jpg"]);
    key(&mut app, Key::F(2), NO);
    assert_eq!(app.rename_text(), Some("photo.jpg"));
    typed(&mut app, "pic");
    assert_eq!(app.rename_text(), Some("pic.jpg"), "only the base name was selected");
    key(&mut app, Key::Escape, NO);
    assert!(!app.is_renaming());
    assert!(ops(&mut app).is_empty());

    key(&mut app, Key::F(2), NO);
    key(&mut app, Key::Char('A'), CTRL);
    typed(&mut app, "con.jpg");
    key(&mut app, Key::Enter, NO);
    assert!(app.is_renaming(), "an invalid name keeps the editor open");
    assert_eq!(app.rename_text(), Some("con.jpg"));
    assert_eq!(app.rename_hint(), Some("This name is reserved by Windows"));
    assert!(ops(&mut app).is_empty());

    key(&mut app, Key::Char('A'), CTRL);
    typed(&mut app, "holiday.jpg");
    key(&mut app, Key::Enter, NO);
    assert!(!app.is_renaming());
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Rename { item: p(r"C:\x\photo.jpg"), name: b"holiday.jpg".to_vec() });
    done(&mut app, id, op, &[(r"C:\x\photo.jpg", r"C:\x\holiday.jpg")]);
    change(&mut app, ChangeKind::Removed, "photo.jpg", false);
    change(&mut app, ChangeKind::Added, "holiday.jpg", false);
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!(v.focus.map(|f| v.listing().name(f).to_vec()), Some(b"holiday.jpg".to_vec()));
    assert_eq!(selected(&app), ["holiday.jpg"]);
}

#[test]
fn clicking_elsewhere_commits_the_rename() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::F(2), NO);
    typed(&mut app, "c");
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    let at = row_xy(&app, 1);
    press(&mut app, at, MouseButton::Left, NO);
    assert!(!app.is_renaming());
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Rename { item: p(r"C:\x\a.txt"), name: b"c.txt".to_vec() }]);
}

fn sent(app: &mut App) -> Vec<(ProviderId, OpId, FileOp)> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { provider, req: ProviderRequest::Op { id, op, .. } } = e { Some((provider, id, op)) } else { None }).collect()
}

fn ubuntu() -> ProviderId {
    ProviderId::Wsl("Ubuntu".into())
}

#[test]
fn rename_in_wsl_sends_the_linux_name_to_the_agent() {
    let mut app = app_at("wsl://Ubuntu/home");
    deliver(&mut app, &[("a", false)]);
    select(&mut app, &["a"]);
    key(&mut app, Key::F(2), NO);
    typed(&mut app, "x:y.");
    key(&mut app, Key::Enter, NO);
    let [(provider, _, op)] = sent(&mut app).try_into().unwrap();
    assert_eq!((provider, op), (ubuntu(), FileOp::Rename { item: p("/home/a"), name: b"x:y.".to_vec() }));
}

#[test]
fn trash_and_delete_in_wsl_go_to_the_distro_with_linux_paths() {
    let mut app = app_at("wsl://Ubuntu/home");
    deliver(&mut app, &[("a:b", false), ("c", true)]);
    select(&mut app, &["a:b", "c"]);
    key(&mut app, Key::Delete, NO);
    let sent_ops: Vec<_> = sent(&mut app).into_iter().map(|(provider, _, op)| (provider, op)).collect();
    assert_eq!(sent_ops, [(ubuntu(), FileOp::Trash(vec![p("/home/a:b"), p("/home/c")]))]);
    assert_eq!(app.job_states()[0].0, "Moving 2 items to the Trash");
    select(&mut app, &["c"]);
    key(&mut app, Key::Delete, SHIFT);
    assert!(app.jobs.modal());
    key(&mut app, Key::Enter, NO);
    let sent_ops: Vec<_> = sent(&mut app).into_iter().map(|(provider, _, op)| (provider, op)).collect();
    assert_eq!(sent_ops, [(ubuntu(), FileOp::Delete(vec![p("/home/c")]))]);
}

#[test]
fn ctrl_z_after_del_in_wsl_restores_from_the_linux_trash() {
    let mut app = app_at("wsl://Ubuntu/home");
    deliver(&mut app, &[("a", false)]);
    select(&mut app, &["a"]);
    key(&mut app, Key::Delete, NO);
    let [(_, id, op)] = sent(&mut app).try_into().unwrap();
    done(&mut app, id, op, &[("/home/a", "/home/u/.local/share/Trash/files/a")]);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('Z'), CTRL);
    let [(provider, id, op)] = sent(&mut app).try_into().unwrap();
    assert_eq!((provider, op.clone()), (ubuntu(), FileOp::Restore(vec![(p("/home/u/.local/share/Trash/files/a"), p("/home/a"))])));
    done(&mut app, id, op, &[("/home/u/.local/share/Trash/files/a", "/home/a")]);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('Y'), CTRL);
    let [(provider, _, op)] = sent(&mut app).try_into().unwrap();
    assert_eq!((provider, op), (ubuntu(), FileOp::Trash(vec![p("/home/a")])));
}

#[test]
fn confirmed_wsl_trash_can_delete_permanently_through_the_agent() {
    let mut app = app_at("wsl://Ubuntu/home");
    deliver(&mut app, &[("a", false)]);
    select(&mut app, &["a"]);
    app.confirm_delete = crate::ui::ConfirmDelete::Always;
    key(&mut app, Key::Delete, NO);
    assert_eq!(app.asking(), Some("confirm"));
    app.jobs.all = true;
    key(&mut app, Key::Enter, NO);
    let [(provider, _, op)] = sent(&mut app).try_into().unwrap();
    assert_eq!((provider, op), (ubuntu(), FileOp::Delete(vec![p("/home/a")])));
}

#[test]
fn operations_inside_one_distro_are_routed_to_its_agent() {
    let local = ProviderId::Local;
    let copy = FileOp::Copy { items: vec![p("\\\\wsl.localhost\\Ubuntu\\home\\a\u{F03A}b")], dest: p(r"\\wsl$\ubuntu\tmp") };
    assert_eq!(route(local.clone(), copy, false), Some((ubuntu(), FileOp::Copy { items: vec![p("/home/a:b")], dest: p("/tmp") })));
    let rename = FileOp::Rename { item: p(r"\\wsl.localhost\Ubuntu\home\a"), name: "x\u{F03A}y".as_bytes().to_vec() };
    assert_eq!(route(local.clone(), rename, false), Some((ubuntu(), FileOp::Rename { item: p("/home/a"), name: b"x:y".to_vec() })));
    let paste = FileOp::Paste { dest: p(r"\\wsl.localhost\Ubuntu\") };
    assert_eq!(route(local.clone(), paste, false), Some((ubuntu(), FileOp::Paste { dest: p("/") })));
    for stays in [
        FileOp::Copy { items: vec![p(r"C:\a")], dest: p(r"\\wsl.localhost\Ubuntu\tmp") },
        FileOp::Move { items: vec![p(r"\\wsl.localhost\Debian\a")], dest: p(r"\\wsl.localhost\Ubuntu\tmp") },
        FileOp::Copy { items: vec![p(r"\\wsl.localhost\Ubuntu\x.zip\a")], dest: p(r"\\wsl.localhost\Ubuntu\tmp") },
        FileOp::Flatten(vec![p(r"\\wsl.localhost\Ubuntu\tmp")]),
        FileOp::EmptyBin,
    ] {
        assert_eq!(route(local.clone(), stays.clone(), false), Some((local.clone(), stays)));
    }
    let back = FileOp::Trash(vec![p(r"\\wsl.localhost\Ubuntu\tmp\a")]);
    assert_eq!(route(ubuntu(), back, false), Some((ubuntu(), FileOp::Trash(vec![p("/tmp/a")]))), "a step recorded from a share report is routed by its paths");
    assert_eq!(route(local.clone(), FileOp::Delete(vec![p("/etc")]), false), None, "Linux paths never go to Windows");
    assert_eq!(route(ubuntu(), FileOp::Copy { items: vec![p("/home/u/a")], dest: p(r"C:\b") }, false), Some((local.clone(), FileOp::Copy { items: vec![p(r"\\wsl.localhost\Ubuntu\home\u\a")], dest: p(r"C:\b") })), "WSL and Windows items go together through the WSL share");
    assert_eq!(route(ubuntu(), FileOp::Copy { items: vec![p(r"C:\a")], dest: p("/tmp") }, false), Some((local.clone(), FileOp::Copy { items: vec![p(r"C:\a")], dest: p(r"\\wsl.localhost\Ubuntu\tmp") })));
    assert_eq!(route(local.clone(), FileOp::Delete(vec![p("/etc")]), true), Some((local.clone(), FileOp::Delete(vec![p("/etc")]))), "a Linux build runs Linux paths locally");
    assert_eq!(route(local.clone(), FileOp::Copy { items: vec![p("/a")], dest: p(r"C:\b") }, true), None);
}

#[test]
fn copy_and_paste_in_wsl_reach_the_agent() {
    let mut app = app_at("wsl://Ubuntu/home");
    deliver(&mut app, &[("a", false)]);
    key(&mut app, Key::Char('V'), CTRL);
    let [(provider, id, op)] = sent(&mut app).try_into().unwrap();
    assert_eq!((provider, op), (ubuntu(), FileOp::Paste { dest: p("/home") }));
    assert_eq!(app.job_states()[0].0, "Pasting into \u{201C}home\u{201D}");
    let report = FileOp::Copy { items: vec![p("/tmp/a")], dest: p("/home") };
    done(&mut app, id, report, &[("/tmp/a", "/home/a (2)")]);
    deliver(&mut app, &[("a", false), ("a (2)", false)]);
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    assert_eq!(selected(&app), ["a (2)"], "the copy in the agent's Linux path is revealed in the WSL tab");
}

#[test]
fn starting_note_replaces_loading_until_the_listing_arrives() {
    let mut app = app_at("wsl://Ubuntu/home");
    let (tab, gen) = (app.tab().id, app.tab().gen);
    app.on_provider_event(ProviderEvent::Status { tab, gen: gen + 1, text: "stale".into() }, t0());
    assert_eq!(app.tab().loading.as_ref().unwrap().note, None);
    app.on_provider_event(ProviderEvent::Status { tab, gen, text: "Starting Ubuntu…".into() }, t0());
    assert_eq!(app.tab().loading.as_ref().unwrap().note.as_deref(), Some("Starting Ubuntu…"));
    deliver(&mut app, &[("a", false)]);
    assert!(app.tab().loading.is_none());
}

#[test]
fn new_folder_picks_a_free_name_and_starts_renaming_it() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("New folder", true), ("new folder (2)", true), ("a.txt", false)]);
    key(&mut app, Key::Char('N'), CTRL_SHIFT);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::NewFolder { dir: p(r"C:\x"), name: b"New folder (3)".to_vec() });
    done(&mut app, id, op, &[(r"C:\x\New folder (3)", r"C:\x\New folder (3)")]);
    assert!(!app.is_renaming());
    change(&mut app, ChangeKind::Added, "New folder (3)", true);
    assert_eq!(app.rename_text(), Some("New folder (3)"));
    assert_eq!(selected(&app), ["New folder (3)"]);
    key(&mut app, Key::Char('Z'), CTRL);
    assert!(ops(&mut app).is_empty(), "keys go to the editor while renaming");
    key(&mut app, Key::Escape, NO);
    key(&mut app, Key::Char('Z'), CTRL);
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Trash(vec![p(r"C:\x\New folder (3)")])]);
}

#[test]
fn file_shortcuts_leave_the_filter_field_alone_and_a_new_folder_clears_the_filter() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Char('F'), CTRL);
    typed(&mut app, "b");
    key(&mut app, Key::Char('C'), CTRL);
    key(&mut app, Key::Char('V'), CTRL);
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::Provider { .. } | Effect::SetClipboardFiles { .. })));
    key(&mut app, Key::Char('N'), CTRL_SHIFT);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::NewFolder { dir: p(r"C:\x"), name: b"New folder".to_vec() });
    done(&mut app, id, op, &[(r"C:\x\New folder", r"C:\x\New folder")]);
    change(&mut app, ChangeKind::Added, "New folder", true);
    assert_eq!(app.rename_text(), Some("New folder"));
}

#[test]
fn nothing_to_place_into_this_pc() {
    let mut app = app_at("This PC");
    deliver(&mut app, &[("C:", true)]);
    key(&mut app, Key::Char('V'), CTRL);
    key(&mut app, Key::Char('N'), CTRL_SHIFT);
    select(&mut app, &["C:"]);
    key(&mut app, Key::Char('C'), CTRL);
    key(&mut app, Key::F(2), NO);
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::Provider { .. } | Effect::SetClipboardFiles { .. })));
    assert!(!app.is_renaming());
}

#[test]
fn right_click_opens_the_shell_menu_and_verbs_come_back() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    let at = row_xy(&app, 1);
    press(&mut app, at, MouseButton::Right, SHIFT);
    release(&mut app, at, MouseButton::Right);
    let menu = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).unwrap();
    assert_eq!((menu.dir, menu.items, menu.extended), (p(r"C:\x"), vec![p(r"C:\x\b.txt")], true));
    let empty = (at.0, app.layout.list.y + 200.0);
    press(&mut app, empty, MouseButton::Right, NO);
    release(&mut app, empty, MouseButton::Right);
    let menu = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).unwrap();
    assert!(menu.items.is_empty() && !menu.extended);
    select(&mut app, &["a.txt"]);
    app.on_event(&Event::ShellVerb("rename".into()), t0());
    assert_eq!(app.rename_text(), Some("a.txt"));
    key(&mut app, Key::Escape, NO);
    key(&mut app, Key::Apps, NO);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::ShellMenu(m) if m.items == [p(r"C:\x\a.txt")])));
}

#[test]
fn alt_enter_shows_properties_of_the_selection_or_the_folder() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    key(&mut app, Key::Enter, ALT);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Properties(p) if p == [FsPath::parse(r"C:\x").unwrap()])));
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Enter, ALT);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Properties(p) if p == [FsPath::parse(r"C:\x\a.txt").unwrap()])));
    key(&mut app, Key::Enter, Mods { shift: true, ..ALT });
    assert_eq!(app.asking(), Some("properties"));
}

#[test]
fn dragging_selected_rows_starts_a_shell_drag_and_a_plain_click_narrows_the_selection() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false), ("c.txt", false)]);
    select(&mut app, &["a.txt", "b.txt"]);
    let (x, y) = row_xy(&app, 1);
    press(&mut app, (x, y), MouseButton::Left, NO);
    assert_eq!(selected(&app), ["a.txt", "b.txt"], "the selection survives the press");
    app.on_event(&Event::MouseMove { x: x + 2.0, y }, t0());
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::StartDrag(_))));
    app.on_event(&Event::MouseMove { x: x + 12.0, y: y + 3.0 }, t0());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::StartDrag(p) if p.len() == 2)));
    press(&mut app, (x, y), MouseButton::Left, NO);
    release(&mut app, (x, y), MouseButton::Left);
    assert_eq!(selected(&app), ["b.txt"]);
}

#[test]
fn ctrl_dragging_a_selected_item_drags_the_selection_and_a_ctrl_click_toggles_on_release() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false), ("c.txt", false)]);
    select(&mut app, &["a.txt"]);
    let (a, b) = (row_xy(&app, 0), row_xy(&app, 1));
    let dragged = |app: &mut App| app.drain_effects().find_map(|e| if let Effect::StartDrag(paths) = e { Some(paths) } else { None });
    press(&mut app, a, MouseButton::Left, CTRL);
    assert_eq!(selected(&app), ["a.txt"], "the only selected item stays selected while it may still be dragged");
    app.on_event(&Event::MouseMove { x: a.0 + 12.0, y: a.1 + 3.0 }, t0());
    assert_eq!(dragged(&mut app), Some(vec![p(r"C:\x\a.txt")]));
    release(&mut app, (a.0 + 12.0, a.1 + 3.0), MouseButton::Left);
    assert_eq!(selected(&app), ["a.txt"], "a drag never toggles");

    press(&mut app, b, MouseButton::Left, CTRL);
    assert_eq!(selected(&app), ["a.txt", "b.txt"], "an unselected item joins on the press");
    release(&mut app, b, MouseButton::Left);
    assert_eq!(selected(&app), ["a.txt", "b.txt"]);
    press(&mut app, b, MouseButton::Left, CTRL);
    app.on_event(&Event::MouseMove { x: b.0 + 12.0, y: b.1 + 3.0 }, t0());
    assert_eq!(dragged(&mut app), Some(vec![p(r"C:\x\a.txt"), p(r"C:\x\b.txt")]), "ctrl+drag copies the whole selection");
    release(&mut app, (b.0 + 12.0, b.1 + 3.0), MouseButton::Left);
    assert_eq!(selected(&app), ["a.txt", "b.txt"]);

    press(&mut app, a, MouseButton::Left, CTRL);
    assert_eq!(selected(&app), ["a.txt", "b.txt"]);
    release(&mut app, a, MouseButton::Left);
    assert_eq!(selected(&app), ["b.txt"], "a ctrl+click without a drag still toggles");
    assert!(dragged(&mut app).is_none());
}

#[test]
fn drops_copy_or_move_by_volume_and_modifiers() {
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("sub", true), ("a.txt", false)]);
    let empty = (300.0, app.layout.list.y + 200.0);
    let all = COPY | MOVE | LINK;
    let enter = |app: &mut App, paths: Vec<FsPath>, at: (f32, f32), mods: Mods| app.drag(&DragEvent::Enter { paths, x: at.0, y: at.1, mods, allowed: all, right: false }, t0());
    assert_eq!(enter(&mut app, vec![p(r"D:\in\f.txt")], empty, NO), COPY);
    assert_eq!(app.drop_spot(), Some(Spot::List));
    assert_eq!(app.drag(&DragEvent::Over { x: empty.0, y: empty.1, mods: SHIFT, allowed: all }, t0()), MOVE);
    assert_eq!(app.drag(&DragEvent::Drop { x: empty.0, y: empty.1, mods: SHIFT, allowed: all }, t0()), MOVE);
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Move { items: vec![p(r"D:\in\f.txt")], dest: p(r"C:\x") }]);
    assert_eq!(app.drop_spot(), None);

    let sub = row_xy(&app, 0);
    assert_eq!(enter(&mut app, vec![p(r"C:\y\g.txt")], sub, NO), MOVE);
    assert_eq!(app.drop_spot(), Some(Spot::Row(0)));
    assert_eq!(app.drop_folder(), Some(p(r"C:\x\sub")), "the folder under the cursor is offered to a source that extracts itself");
    assert_eq!(app.drag(&DragEvent::Drop { x: sub.0, y: sub.1, mods: CTRL, allowed: all }, t0()), COPY);
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Copy { items: vec![p(r"C:\y\g.txt")], dest: p(r"C:\x\sub") }]);

    assert_eq!(enter(&mut app, vec![p(r"C:\x\a.txt")], empty, NO), NONE, "moving into its own folder does nothing");
    assert_eq!(app.drag(&DragEvent::Over { x: empty.0, y: empty.1, mods: ALT, allowed: all }, t0()), LINK);
    assert_eq!(app.drop_folder(), None, "a link is never handed to the source");
    assert_eq!(app.drag(&DragEvent::Over { x: sub.0, y: sub.1, mods: NO, allowed: all }, t0()), MOVE);
    app.drag(&DragEvent::Leave, t0());
    assert_eq!(enter(&mut app, vec![p(r"C:\x\sub")], sub, NO), NONE, "a folder cannot go into itself");
    app.drag(&DragEvent::Leave, t0());
    assert_eq!(enter(&mut app, vec![p(r"C:\x")], sub, CTRL), NONE, "nor into its own subfolder");
    app.drag(&DragEvent::Leave, t0());
    assert!(ops(&mut app).is_empty());
}

#[test]
fn dropping_on_the_title_bar_reveals_the_file_like_before() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    let title = (app.layout.new_tab.right() + 40.0, 10.0);
    let all = drop_effect::COPY | drop_effect::MOVE;
    assert_eq!(app.drag(&DragEvent::Enter { paths: vec![p(r"D:\docs\r.txt")], x: title.0, y: title.1, mods: NO, allowed: all, right: false }, t0()), drop_effect::COPY);
    assert_eq!(app.drop_spot(), None);
    app.drag(&DragEvent::Drop { x: title.0, y: title.1, mods: NO, allowed: all }, t0());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { path, .. }, .. } if path == p(r"D:\docs"))));
    assert_eq!(app.current_location(), &Location::parse(r"D:\docs").unwrap());
}

fn drop_menu(app: &mut App) -> Vec<MenuItem> {
    app.drain_effects()
        .find_map(|e| match e {
            Effect::ShellMenu(m) if m.items.is_empty() && !m.shell => Some(m.menu),
            _ => None,
        })
        .expect("a menu at the drop point")
}

fn drop_rows(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Command { label, checked, disabled, .. } => format!("{label}{}{}", if *checked { " *" } else { "" }, if *disabled { " (off)" } else { "" }),
            _ => "-".into(),
        })
        .collect()
}

#[test]
fn right_dragging_rows_asks_what_to_do_where_they_land() {
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("sub", true), ("a.txt", false), ("b.txt", false)]);
    let (a, sub) = (row_xy(&app, 1), row_xy(&app, 0));
    press(&mut app, a, MouseButton::Right, NO);
    assert_eq!(selected(&app), ["a.txt"], "a right press selects the row like a click");
    app.on_event(&Event::MouseMove { x: a.0 + 12.0, y: a.1 + 3.0 }, t0());
    let dragged = vec![p(r"C:\x\a.txt")];
    assert!(app.drain_effects().any(|e| matches!(&e, Effect::StartDrag(p) if *p == dragged)));
    release(&mut app, a, MouseButton::Right);
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::ShellMenu(_))), "letting go after a drag never opens the context menu");

    let all = COPY | MOVE;
    assert_eq!(app.drag(&DragEvent::Enter { paths: dragged.clone(), x: sub.0, y: sub.1, mods: NO, allowed: all, right: false }, t0()), MOVE, "the drag shows what a left drop would do");
    assert_eq!(app.drop_spot(), Some(Spot::Row(0)));
    assert_eq!(app.drag(&DragEvent::Drop { x: sub.0, y: sub.1, mods: NO, allowed: all }, t0()), COPY);
    let menu = drop_menu(&mut app);
    assert_eq!(drop_rows(&menu), ["Copy here", "Move here *", "Create shortcut here", "Symbolic link (off)", "Junction (off)", "Hard link (off)", "-", "Cancel"], "Flit offers every choice for its own drag");
    assert!(op_list(&mut app).is_empty(), "nothing happens before a choice");
    run_menu(&mut app, &menu, "Move here");
    assert_eq!(op_list(&mut app), [FileOp::Move { items: dragged.clone(), dest: p(r"C:\x\sub") }]);

    app.drag(&DragEvent::Enter { paths: dragged.clone(), x: sub.0, y: sub.1, mods: NO, allowed: all, right: false }, t0());
    app.drag(&DragEvent::Drop { x: sub.0, y: sub.1, mods: NO, allowed: all }, t0());
    assert_eq!(op_list(&mut app), [FileOp::Move { items: dragged, dest: p(r"C:\x\sub") }], "a later left drag of the same files drops at once");
}

#[test]
fn a_right_drop_from_another_program_offers_what_it_allows() {
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("sub", true), ("a.txt", false)]);
    let (sub, empty, title) = (row_xy(&app, 0), (300.0, app.layout.list.y + 200.0), (app.layout.new_tab.right() + 40.0, 10.0));
    let right_drop = |app: &mut App, paths: &[&str], at: (f32, f32), allowed: u32| {
        let paths = paths.iter().map(|s| p(s)).collect();
        let shown = app.drag(&DragEvent::Enter { paths, x: at.0, y: at.1, mods: NO, allowed, right: true }, t0());
        (shown, app.drag(&DragEvent::Drop { x: at.0, y: at.1, mods: NO, allowed }, t0()))
    };
    assert_eq!(right_drop(&mut app, &[r"D:\in\f.txt"], sub, COPY | MOVE | LINK), (COPY, COPY));
    let menu = drop_menu(&mut app);
    assert_eq!(drop_rows(&menu), ["Copy here *", "Move here", "Create shortcut here", "Symbolic link (off)", "Junction (off)", "Hard link (off)", "-", "Cancel"]);
    run_menu(&mut app, &menu, "Create shortcut here");
    assert_eq!(op_list(&mut app), [FileOp::Link { items: vec![p(r"D:\in\f.txt")], dest: p(r"C:\x\sub") }]);

    right_drop(&mut app, &[r"D:\in\f.txt"], sub, COPY | MOVE | LINK);
    let menu = drop_menu(&mut app);
    run_menu(&mut app, &menu, "Copy here");
    assert_eq!(op_list(&mut app), [FileOp::Copy { items: vec![p(r"D:\in\f.txt")], dest: p(r"C:\x\sub") }]);

    right_drop(&mut app, &[r"D:\in\f.txt"], sub, COPY | MOVE | LINK);
    let menu = drop_menu(&mut app);
    run_menu(&mut app, &menu, "Cancel");
    assert!(op_list(&mut app).is_empty());

    assert_eq!(right_drop(&mut app, &[r"C:\x\a.txt"], empty, COPY | MOVE), (COPY, COPY), "files already here can still be copied");
    assert_eq!(drop_rows(&drop_menu(&mut app)), ["Copy here *", "Move here (off)", "Create shortcut here (off)", "Symbolic link (off)", "Junction (off)", "Hard link (off)", "-", "Cancel"]);

    app.caps = crate::caps::Caps { unix_paths: true };
    right_drop(&mut app, &[r"D:\in\f.txt"], sub, COPY | MOVE | LINK);
    assert_eq!(drop_rows(&drop_menu(&mut app)), ["Copy here *", "Move here", "Create link here", "-", "Cancel"], "unix offers symbolic links under their own name");

    assert_eq!(right_drop(&mut app, &[r"D:\in\f.txt"], title, COPY), (COPY, COPY), "the title bar still reveals at once");
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::ShellMenu(_))));
}

#[test]
fn right_drop_links_use_the_existing_job_queue_select_the_created_item_and_undo_only_the_link() {
    use crate::provider::{LinkChoices, LinkKind};
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("keep.txt", false)]);
    let at = (300.0, app.layout.list.y + 200.0);
    let source = p(r"C:\from\a.txt");
    let allowed = COPY | MOVE | LINK;
    app.drag(&DragEvent::Enter { paths: vec![source.clone()], x: at.0, y: at.1, mods: NO, allowed, right: true }, t0());
    app.drag(&DragEvent::Drop { x: at.0, y: at.1, mods: NO, allowed }, t0());
    let effects: Vec<_> = app.drain_effects().collect();
    let generation = effects.iter().find_map(|e| if let Effect::LinkChoices { generation, items, dest } = e {
        assert_eq!(items.as_ref().unwrap(), std::slice::from_ref(&source));
        assert_eq!(dest, &p(r"C:\x"));
        Some(*generation)
    } else { None }).unwrap();
    let menu = effects.into_iter().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).unwrap();
    app.on_event(&Event::LinkChoices { generation, choices: LinkChoices([true, true, false, true]) }, t0());
    run_menu(&mut app, &menu, "Hard link");
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::LinkAs { items: vec![source], dest: p(r"C:\x"), kind: LinkKind::Hard });
    assert_eq!(app.running_jobs(), 1);
    done(&mut app, id, op, &[(r"C:\from\a.txt", r"C:\x\Link to a.txt")]);
    deliver(&mut app, &[("keep.txt", false), ("Link to a.txt", false)]);
    app.frame(&mut Gfx::new(Mono, 512), t0());
    assert_eq!(selected(&app), ["Link to a.txt"]);
    key(&mut app, Key::Char('Z'), CTRL);
    assert_eq!(op_list(&mut app), [FileOp::Delete(vec![p(r"C:\x\Link to a.txt")])]);
}

#[test]
fn cancelling_a_right_drop_or_denying_link_transfer_never_uses_old_link_choices() {
    use crate::provider::{LinkChoices, LinkKind};
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[]);
    let asked = || Asked { paths: vec![p(r"C:\a.txt")], dest: p(r"C:\x") };
    app.ask_drop(asked(), drop_effect::COPY | drop_effect::LINK, drop_effect::COPY, (300.0, 200.0));
    let generation = app.ops.link_probe.as_ref().unwrap().generation;
    app.on_link_choices(generation, LinkChoices([true; 4]));
    app.drop_chosen(drop_effect::NONE);
    app.drop_as(LinkKind::Hard);
    assert!(ops(&mut app).is_empty());
    app.ask_drop(asked(), drop_effect::COPY, drop_effect::COPY, (300.0, 200.0));
    app.on_link_choices(generation, LinkChoices([true; 4]));
    app.drop_as(LinkKind::Hard);
    assert!(ops(&mut app).is_empty());
}

#[test]
fn the_shortcut_choice_keeps_legacy_undo_and_redo_through_the_recycle_bin() {
    use crate::provider::LinkKind;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[]);
    app.paste_as(LinkKind::Shortcut);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::PasteLink { dest: p(r"C:\x") });
    let link = FileOp::Link { items: vec![p(r"C:\source")], dest: p(r"C:\x") };
    done(&mut app, id, link, &[(r"C:\source", r"C:\x\Link to source.lnk")]);
    key(&mut app, Key::Char('Z'), CTRL);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Trash(vec![p(r"C:\x\Link to source.lnk")]));
    done(&mut app, id, op, &[(r"C:\x\Link to source.lnk", r"C:\$Recycle.Bin\S\$Rlink")]);
    key(&mut app, Key::Char('Y'), CTRL);
    assert_eq!(op_list(&mut app), [FileOp::Restore(vec![(p(r"C:\$Recycle.Bin\S\$Rlink"), p(r"C:\x\Link to source.lnk"))])]);
}

#[test]
fn a_right_click_without_moving_still_opens_the_context_menu() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    let a = row_xy(&app, 0);
    press(&mut app, a, MouseButton::Right, NO);
    app.on_event(&Event::MouseMove { x: a.0 + 2.0, y: a.1 }, t0());
    release(&mut app, a, MouseButton::Right);
    let effects: Vec<Effect> = app.drain_effects().collect();
    assert!(!effects.iter().any(|e| matches!(e, Effect::StartDrag(_))));
    assert!(effects.iter().any(|e| matches!(e, Effect::ShellMenu(m) if m.items == [p(r"C:\x\a.txt")])));
}

#[test]
fn content_without_file_paths_lands_in_the_folder_under_the_cursor() {
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("sub", true), ("a.txt", false)]);
    let (sub, title) = (row_xy(&app, 0), (app.layout.new_tab.right() + 40.0, 10.0));
    let enter = |app: &mut App, at: (f32, f32), allowed: u32| app.drag(&DragEvent::Enter { paths: Vec::new(), x: at.0, y: at.1, mods: NO, allowed, right: false }, t0());
    assert_eq!(enter(&mut app, sub, COPY | MOVE), COPY);
    assert_eq!((app.drop_folder(), app.drop_caption()), (Some(p(r"C:\x\sub")), Some(("Copy to %1".to_owned(), "sub".to_owned()))), "the platform learns where to put it");
    assert_eq!(app.drag(&DragEvent::Over { x: sub.0, y: sub.1, mods: SHIFT, allowed: COPY | MOVE }, t0()), MOVE);
    assert_eq!(app.drag(&DragEvent::Drop { x: sub.0, y: sub.1, mods: NO, allowed: COPY | MOVE }, t0()), COPY);
    assert_eq!(op_list(&mut app), [FileOp::PasteDropped { dest: p(r"C:\x\sub"), moving: false }], "the content is saved by a queued operation that selects what it made");
    enter(&mut app, sub, COPY | MOVE);
    app.drag(&DragEvent::Drop { x: sub.0, y: sub.1, mods: SHIFT, allowed: COPY | MOVE }, t0());
    assert_eq!(op_list(&mut app), [FileOp::PasteDropped { dest: p(r"C:\x\sub"), moving: true }]);
    assert_eq!(enter(&mut app, title, COPY), NONE, "content cannot be revealed");
    app.drag(&DragEvent::Leave, t0());
    assert_eq!(enter(&mut app, sub, LINK), NONE);
    app.drag(&DragEvent::Leave, t0());
}

fn two_tabs() -> App {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("sub", true), ("a.txt", false)]);
    app.dispatch(vec![crate::open::Route { folder: Location::parse(r"D:\docs").unwrap(), select: None }], crate::open::OpenTarget::NewTab, t0());
    app.select_tab(0);
    app.set_pinned(vec![Location::parse(r"E:\pinned").unwrap()]);
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    app.drain_effects().for_each(drop);
    app
}

fn center(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

type Dropped = (u32, Option<Spot>, Option<(String, String)>, Vec<FileOp>);

fn drop_on(app: &mut App, at: (f32, f32), paths: &[&str]) -> Dropped {
    let all = drop_effect::COPY | drop_effect::MOVE | drop_effect::LINK;
    let paths = paths.iter().map(|s| p(s)).collect();
    let effect = app.drag(&DragEvent::Enter { paths, x: at.0, y: at.1, mods: NO, allowed: all, right: false }, t0());
    let (spot, caption) = (app.drop_spot(), app.drop_caption());
    app.drag(&DragEvent::Drop { x: at.0, y: at.1, mods: NO, allowed: all }, t0());
    (effect, spot, caption, op_list(app))
}

#[test]
fn tabs_crumbs_and_sidebar_items_take_drops_into_their_folders() {
    use drop_effect::*;
    let mut app = two_tabs();
    let caption = |verb: &str, name: &str| Some((verb.to_owned(), name.to_owned()));
    let tab = center(app.layout.tabs[1].0);
    assert_eq!(drop_on(&mut app, tab, &[r"C:\y\g.txt"]), (COPY, Some(Spot::Tab(1)), caption("Copy to %1", "docs"), vec![FileOp::Copy { items: vec![p(r"C:\y\g.txt")], dest: p(r"D:\docs") }]));
    assert_eq!(app.current_location(), &Location::parse(r"C:\x").unwrap(), "a drop on a tab does not switch to it");
    let crumbs = app.layout.crumbs.clone();
    assert_eq!(app.tab().crumbs.len(), 3);
    let (_, spot, caption_c, ops) = drop_on(&mut app, center(crumbs[1]), &[r"C:\y\g.txt"]);
    assert_eq!((spot, caption_c.map(|c| c.0), ops), (Some(Spot::Crumb(1)), Some("Move to %1".to_owned()), vec![FileOp::Move { items: vec![p(r"C:\y\g.txt")], dest: p(r"C:\") }]));
    assert_eq!(drop_on(&mut app, center(crumbs[0]), &[r"C:\y\g.txt"]).0, NONE, "nothing can be placed in This PC");
    let k = app.nav.sidebar.items.iter().position(|it| it.loc == Location::parse(r"E:\pinned").unwrap()).unwrap();
    let side = center(app.nav.sidebar.rects[k]);
    assert_eq!(drop_on(&mut app, side, &[r"C:\y\g.txt"]), (COPY, Some(Spot::Side(k)), caption("Copy to %1", "pinned"), vec![FileOp::Copy { items: vec![p(r"C:\y\g.txt")], dest: p(r"E:\pinned") }]));
    let row = row_xy(&app, 1);
    assert_eq!(drop_on(&mut app, row, &[r"D:\in\f.txt"]).2, caption("Copy to %1", "x"), "a file row stands for its folder");
    assert_eq!(app.drop_caption(), None);
}

#[test]
fn hovering_a_folder_tab_or_crumb_while_dragging_opens_it() {
    let mut app = two_tabs();
    let all = drop_effect::COPY | drop_effect::MOVE;
    let start = t0();
    let at = |app: &mut App, xy: (f32, f32), ms: u64| app.drag(&DragEvent::Over { x: xy.0, y: xy.1, mods: NO, allowed: all }, start + Duration::from_millis(ms));
    let sub = row_xy(&app, 0);
    app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: sub.0, y: sub.1, mods: NO, allowed: all, right: false }, start);
    at(&mut app, sub, 0);
    at(&mut app, sub, 1200);
    assert_eq!(app.current_location(), &Location::parse(r"C:\x").unwrap());
    at(&mut app, sub, 1350);
    assert_eq!(app.current_location(), &Location::parse(r"C:\x\sub").unwrap(), "the folder under the cursor opens and the drag goes on");
    deliver(&mut app, &[("deeper", true)]);
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    let crumb = center(app.layout.crumbs[1]);
    at(&mut app, crumb, 1400);
    at(&mut app, sub, 2000);
    at(&mut app, crumb, 2100);
    at(&mut app, crumb, 3300);
    assert_eq!(app.current_location(), &Location::parse(r"C:\x\sub").unwrap(), "leaving the spot restarts the wait");
    at(&mut app, crumb, 3450);
    assert_eq!(app.current_location(), &Location::parse(r"C:\").unwrap());
    let tab = center(app.layout.tabs[1].0);
    at(&mut app, tab, 3500);
    at(&mut app, tab, 4900);
    assert_eq!(app.cur, 1);
    app.select_tab(0);
    deliver(&mut app, &[("folder", true)]);
    let row = row_xy(&app, 0);
    at(&mut app, row, 5000);
    app.drag(&DragEvent::Drop { x: row.0, y: row.1, mods: NO, allowed: all }, start + Duration::from_millis(9000));
    assert_eq!(app.current_location(), &Location::parse(r"C:\").unwrap(), "a drop never springs");
    assert_eq!(op_list(&mut app), [FileOp::Copy { items: vec![p(r"D:\in\f.txt")], dest: p(r"C:\folder") }]);
}

#[test]
fn the_spring_delay_setting_decides_when_a_hovered_folder_opens_or_turns_it_off() {
    let all = drop_effect::COPY | drop_effect::MOVE;
    let start = t0();
    let hover = |delay: i64, held: u64| {
        let mut app = two_tabs();
        let i = super::super::settings::index("files.spring_delay").unwrap();
        assert!(super::super::settings::SETTINGS[i].set(&mut app, super::super::settings::Value::Number(delay)));
        let sub = row_xy(&app, 0);
        app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: sub.0, y: sub.1, mods: NO, allowed: all, right: false }, start);
        for ms in [0, held] {
            app.drag(&DragEvent::Over { x: sub.0, y: sub.1, mods: NO, allowed: all }, start + Duration::from_millis(ms));
        }
        app.current_location() == &Location::parse(r"C:\x\sub").unwrap()
    };
    assert!(!hover(1300, 1200) && hover(1300, 1300), "the default waits 1.3 s");
    assert!(!hover(300, 250) && hover(300, 300));
    assert!(!hover(3000, 2900) && hover(3000, 3000));
    assert!(!hover(0, 60_000), "off never springs");
    let i = super::super::settings::index("files.spring_delay").unwrap();
    assert_eq!(super::super::settings::defaults()[i], super::super::settings::Value::Number(1300));
    assert!(!super::super::settings::SETTINGS[i].set(&mut two_tabs(), super::super::settings::Value::Number(3100)));
}

const BIN_REPORT: (&str, &str) = (r"C:\$Recycle.Bin\S-1\$RAB12CD.txt", r"C:\Users\u\Docs\report.txt");
const BIN_OLD: (&str, &str) = (r"D:\$Recycle.Bin\S-1\$R000001", r"D:\old");

fn bin_listing(items: &[(&str, &str)]) -> (Listing, SortKeys) {
    let mut go = |_: &crate::listing::ListingBuilder<'_>| true;
    let mut b = crate::listing::ListingBuilder::new(CaseMode::Insensitive, &mut go);
    for &(recycled, original) in items {
        let o = p(original);
        let name: Vec<u16> = String::from_utf8_lossy(o.file_name()).encode_utf16().collect();
        let attrs = if original.ends_with("old") { attr::DIR } else { 0 } | attr::SIZE_KNOWN;
        recycle::push(&mut b, &name, Meta { attrs, size: 5, modified: 7, created: 7 }, &p(recycled), &o.parent().unwrap());
    }
    (std::mem::take(&mut b.listing), std::mem::replace(&mut b.keys, SortKeys::new(CaseMode::Insensitive)))
}

fn deliver_bin(app: &mut App, items: &[(&str, &str)]) {
    let t = app.tab();
    let (listing, keys) = bin_listing(items);
    let view = t.view_spec;
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    app.drain_effects().for_each(drop);
}

fn pairs(items: &[(&str, &str)]) -> Vec<(FsPath, FsPath)> {
    items.iter().map(|&(r, o)| (p(r), p(o))).collect()
}

fn bin_app() -> App {
    let mut app = app_at("shell:RecycleBinFolder");
    deliver_bin(&mut app, &[BIN_REPORT, BIN_OLD]);
    app
}

fn op_list(app: &mut App) -> Vec<FileOp> {
    ops(app).into_iter().map(|(_, op)| op).collect()
}

#[test]
fn the_recycle_bin_shows_where_items_came_from_and_refuses_ordinary_file_commands() {
    let mut app = bin_app();
    assert_eq!(app.columns(), super::super::BIN_COLUMNS);
    assert_eq!(app.tab().crumbs.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), ["Recycle Bin"]);
    let v = app.tab().view.as_ref().unwrap();
    let names: Vec<&[u8]> = v.rows().iter().map(|&i| v.listing().name(i)).collect();
    assert_eq!(names, [&b"old"[..], b"report.txt"]);
    let report = v.rows()[1];
    assert_eq!(app.tab().entry_location(report), Location::parse(BIN_REPORT.1).unwrap());
    select(&mut app, &["report.txt"]);
    key(&mut app, Key::Enter, NO);
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::Provider { req: ProviderRequest::Open { .. } | ProviderRequest::List { .. }, .. })));
    assert_eq!(app.tab().error.as_deref(), Some("\"report.txt\" is in the Recycle Bin. Restore it to open it."));
    key(&mut app, Key::Char('C'), CTRL);
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::SetClipboardFiles { .. })));
    assert_eq!(app.tab().error.as_deref(), Some(BIN_ONLY.t()));
    key(&mut app, Key::Char('X'), CTRL);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::SetClipboardFiles { paths, cut: true } if paths == [p(BIN_REPORT.0)])), "a cut hands the recycled item to the shell");
    key(&mut app, Key::Enter, ALT);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Properties(paths) if paths == [p(BIN_REPORT.0)])), "properties are the shell's sheet for the recycled item");
    key(&mut app, Key::F(10), SHIFT);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::ShellMenu(m) if !m.shell && !m.menu.is_empty())));
    for (k, m) in [(Key::Char('V'), CTRL), (Key::Char('N'), CTRL_SHIFT)] {
        key(&mut app, k, m);
        assert!(ops(&mut app).is_empty());
        assert_eq!(app.tab().error.as_deref(), Some("Files cannot be placed in the Recycle Bin"));
    }
    key(&mut app, Key::F(2), NO);
    assert!(!app.is_renaming());
    let row = row_xy(&app, 1);
    press(&mut app, row, MouseButton::Left, NO);
    app.on_event(&Event::MouseMove { x: row.0 + 40.0, y: row.1 + 40.0 }, t0());
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::StartDrag(_))), "bin items are not dragged out as files");
}

#[test]
fn delete_in_the_bin_purges_and_the_bin_commands_restore_or_empty() {
    let mut app = bin_app();
    select(&mut app, &["report.txt"]);
    for m in [NO, SHIFT] {
        key(&mut app, Key::Delete, m);
        assert_eq!(app.asking(), Some("confirm"));
        assert!(op_list(&mut app).is_empty(), "nothing is purged before the confirmation");
        key(&mut app, Key::Enter, NO);
        assert_eq!(op_list(&mut app), [FileOp::Purge(pairs(&[BIN_REPORT]))]);
    }
    assert!(app.run_action("bin.restore", t0()));
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Restore(pairs(&[BIN_REPORT])));
    done(&mut app, id, op, &[BIN_REPORT]);
    key(&mut app, Key::Char('Z'), CTRL);
    assert_eq!(op_list(&mut app), [FileOp::Trash(vec![p(BIN_REPORT.1)])], "a restore is undone by recycling again");
    app.run_action("bin.restore_all", t0());
    assert_eq!(app.asking(), Some("confirm"));
    assert!(op_list(&mut app).is_empty(), "nothing is restored before the confirmation");
    key(&mut app, Key::Escape, NO);
    assert!(app.asking().is_none() && op_list(&mut app).is_empty(), "a declined confirmation restores nothing");
    app.run_action("bin.restore_all", t0());
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::Restore(pairs(&[BIN_REPORT, BIN_OLD]))]);
    app.run_action("bin.restore", t0());
    assert!(app.asking().is_none(), "restoring the selection does not ask");
    op_list(&mut app);
    app.run_action("bin.empty", t0());
    key(&mut app, Key::Escape, NO);
    assert!(app.asking().is_none() && op_list(&mut app).is_empty(), "a declined confirmation deletes nothing");
    app.run_action("bin.empty", t0());
    key(&mut app, Key::Enter, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::EmptyBin);
    done(&mut app, id, op, &[]);
    select(&mut app, &[]);
    app.run_action("bin.restore", t0());
    assert!(ops(&mut app).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some("Select the items to restore"));
}

fn menu_rows(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Command { label, .. } => label.clone(),
            MenuItem::Submenu { label, .. } => format!("{label} >"),
            MenuItem::Header(_) | MenuItem::Separator => "-".into(),
        })
        .collect()
}

fn run_menu(app: &mut App, items: &[MenuItem], label: &str) {
    fn flat(items: &[MenuItem], out: &mut Vec<String>) {
        for i in items {
            match i {
                MenuItem::Command { label, .. } => out.push(label.clone()),
                MenuItem::Submenu { items, .. } => flat(items, out),
                MenuItem::Header(_) | MenuItem::Separator => {}
            }
        }
    }
    let mut all = Vec::new();
    flat(items, &mut all);
    let k = all.iter().position(|l| l == label).unwrap_or_else(|| panic!("{label} not in {all:?}"));
    app.on_event(&Event::MenuCommand(k), t0());
}

#[test]
fn right_click_in_the_bin_offers_what_explorer_offers_for_one_or_many_items() {
    let mut app = bin_app();
    for picked in [&["report.txt"][..], &["report.txt", "old"]] {
        select(&mut app, picked);
        let menu = app.build_menu(true);
        assert_eq!(menu_rows(&menu), ["Restore", "-", "Cut", "-", "Delete", "-", "Properties"], "{picked:?}");
        let want: Vec<(&str, &str)> = [BIN_REPORT, BIN_OLD].into_iter().filter(|(_, o)| picked.iter().any(|n| o.ends_with(n))).collect();
        let recycled: Vec<FsPath> = want.iter().map(|(r, _)| p(r)).collect();
        run_menu(&mut app, &menu, "Restore");
        assert_eq!(op_list(&mut app), [FileOp::Restore(pairs(&want))]);
        let menu = app.build_menu(true);
        run_menu(&mut app, &menu, "Delete");
        assert_eq!(app.asking(), Some("confirm"), "a permanent delete asks first");
        assert!(op_list(&mut app).is_empty());
        key(&mut app, Key::Enter, NO);
        assert_eq!(op_list(&mut app), [FileOp::Purge(pairs(&want))]);
        let menu = app.build_menu(true);
        run_menu(&mut app, &menu, "Cut");
        assert!(app.drain_effects().any(|e| matches!(&e, Effect::SetClipboardFiles { paths, cut: true } if *paths == recycled)));
        let menu = app.build_menu(true);
        run_menu(&mut app, &menu, "Properties");
        assert!(app.drain_effects().any(|e| matches!(&e, Effect::Properties(paths) if *paths == recycled)));
        assert!(app.asking().is_none(), "the shell's sheet, not Flit's own dialog");
    }
}

#[test]
fn right_click_on_the_bin_background_empties_restores_sorts_and_shows_properties() {
    let mut app = bin_app();
    select(&mut app, &[]);
    let menu = app.build_menu(false);
    let rows = menu_rows(&menu);
    assert_eq!(rows[..3], ["Sort >", "Group >", "Refresh"], "{rows:?}");
    let tail = &rows[rows.len() - 4..];
    assert_eq!(tail, ["Empty Recycle Bin", "Restore all items", "-", "Properties"], "{rows:?}");
    let MenuItem::Submenu { items: sorts, .. } = &menu[0] else { panic!() };
    assert!(menu_rows(sorts).iter().any(|l| l == "Original Location"), "{:?}", menu_rows(sorts));
    run_menu(&mut app, &menu, "Restore all items");
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::Restore(pairs(&[BIN_REPORT, BIN_OLD]))]);
    let menu = app.build_menu(false);
    run_menu(&mut app, &menu, "Empty Recycle Bin");
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::EmptyBin]);
    let menu = app.build_menu(false);
    run_menu(&mut app, &menu, "Properties");
    assert!(app.drain_effects().any(|e| matches!(&e, Effect::Properties(paths) if *paths == [FsPath::recycle_bin()])));
}

#[test]
fn the_status_bar_buttons_and_headers_of_the_bin_work_with_the_mouse() {
    let mut app = bin_app();
    select(&mut app, &["old"]);
    let buttons = app.layout.bin;
    assert!(buttons.iter().all(|r| r.w > 0.0) && app.layout.encoding.w == 0.0);
    for (k, want) in [(0, FileOp::Restore(pairs(&[BIN_OLD]))), (1, FileOp::Restore(pairs(&[BIN_REPORT, BIN_OLD]))), (2, FileOp::EmptyBin)] {
        let at = (buttons[k].x + 4.0, buttons[k].y + 4.0);
        press(&mut app, at, MouseButton::Left, NO);
        release(&mut app, at, MouseButton::Left);
        if k > 0 {
            assert_eq!(app.asking(), Some("confirm"));
            key(&mut app, Key::Enter, NO);
        }
        assert_eq!(op_list(&mut app), [want]);
    }
    let origin = app.layout.cols[1];
    let at = (origin.x + 20.0, origin.y + origin.h / 2.0);
    press(&mut app, at, MouseButton::Left, NO);
    release(&mut app, at, MouseButton::Left);
    assert_eq!(app.tab().view_spec.sort.field, crate::sort::SortField::Origin);
    app.navigate(Location::parse(r"C:\x").unwrap(), t0());
    assert_eq!(app.tab().view_spec.sort, crate::sort::SortSpec::default(), "the bin-only sort does not follow the tab out of the bin");
}

#[test]
fn shift_delete_deletes_permanently_without_an_undo_step() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Delete, SHIFT);
    key(&mut app, Key::Escape, NO);
    assert!(ops(&mut app).is_empty() && app.asking().is_none());
    key(&mut app, Key::Delete, SHIFT);
    key(&mut app, Key::Enter, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Delete(vec![p(r"C:\x\a.txt")]));
    done(&mut app, id, op, &[(r"C:\x\a.txt", r"C:\x\a.txt")]);
    key(&mut app, Key::Char('Z'), CTRL);
    assert!(ops(&mut app).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some("Nothing to undo"));
    app.run_action("bin.restore", t0());
    assert!(ops(&mut app).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some(OPEN_BIN.t()));
    app.run_action("bin.empty", t0());
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::EmptyBin], "the bin can be emptied from anywhere");
}

#[test]
fn files_dropped_on_the_recycle_bin_are_trashed() {
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    app.set_places(Arc::new(crate::ui::Places { recycle_bin: true, ..Default::default() }));
    deliver(&mut app, &[("a.txt", false)]);
    let k = app.nav.sidebar.items.iter().position(|it| it.loc.is_recycle_bin()).unwrap();
    let r = app.nav.sidebar.rects[k];
    let at = (r.x + 40.0, r.y + r.h / 2.0);
    let all = COPY | MOVE | LINK;
    let enter = |app: &mut App, paths: Vec<FsPath>, at: (f32, f32)| app.drag(&DragEvent::Enter { paths, x: at.0, y: at.1, mods: CTRL, allowed: all, right: false }, t0());
    assert_eq!(enter(&mut app, vec![p(r"D:\in\f.txt")], at), MOVE);
    assert_eq!(app.drop_spot(), Some(Spot::Side(k)));
    assert_eq!(app.drag(&DragEvent::Drop { x: at.0, y: at.1, mods: NO, allowed: all }, t0()), MOVE);
    assert_eq!(op_list(&mut app), [FileOp::Trash(vec![p(r"D:\in\f.txt")])]);
    assert_eq!(enter(&mut app, vec![p(BIN_REPORT.0)], at), NONE, "what is already in the bin stays there");
    app.drag(&DragEvent::Leave, t0());
    assert_eq!(app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: at.0, y: at.1, mods: NO, allowed: COPY, right: false }, t0()), NONE);
    app.drag(&DragEvent::Leave, t0());

    let mut app = bin_app();
    let row = row_xy(&app, 0);
    assert_eq!(enter(&mut app, vec![p(r"C:\y\g.txt")], row), MOVE, "a drop on a folder in the bin trashes instead of moving into it");
    assert_eq!(app.drop_spot(), Some(Spot::List));
    app.drag(&DragEvent::Drop { x: row.0, y: row.1, mods: NO, allowed: all }, t0());
    assert_eq!(op_list(&mut app), [FileOp::Trash(vec![p(r"C:\y\g.txt")])]);
}

const CTRL_ALT: Mods = Mods { ctrl: true, shift: false, alt: true };

fn launches(app: &mut App) -> Vec<Launch> {
    app.drain_effects()
        .filter_map(|e| match e {
            Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { launch, .. } } => Some(launch),
            _ => None,
        })
        .collect()
}

fn copied(app: &mut App) -> Option<String> {
    app.drain_effects().find_map(|e| if let Effect::SetClipboard(s) = e { Some(s) } else { None })
}

fn with_templates(app: &mut App) {
    let t = |label: &str, ext: &str, template| NewTemplate { label: label.into(), ext: ext.into(), stem: format!("New {label}"), template, names: None };
    let templates = vec![t("Text Document", ".txt", Template::Empty), t("Compressed (zipped) Folder", ".zip", Template::Data(vec![0x50, 0x4B, 5, 6]))];
    app.set_places(Arc::new(crate::ui::Places { templates, ..Default::default() }));
}

#[test]
fn batch_names_keep_extensions_and_skip_taken_names() {
    let items: [(&[u8], bool); 3] = [(b"a.jpg", false), (b"b.PNG", false), (b"dir.v2", true)];
    let names = batch_names("Trip", &items, |n| n == b"trip (2).png");
    assert_eq!(names, [b"Trip (1).jpg".to_vec(), b"Trip (3).PNG".to_vec(), b"Trip (4)".to_vec()]);
    assert!(changes_extension("a.txt", "a.md"));
    assert!(changes_extension("README", "README.md"));
    assert!(changes_extension("a.txt", "a"));
    assert!(!changes_extension("a.txt", "b.TXT"));
    assert!(!changes_extension(".gitignore", ".npmignore"));
}

#[test]
fn f2_with_several_items_selected_renames_them_all_with_numbers() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.jpg", false), ("b.png", false), ("c", true), ("Trip (2).jpg", false)]);
    select(&mut app, &["c", "a.jpg", "b.png"]);
    key(&mut app, Key::F(2), NO);
    assert_eq!(app.rename_text(), Some("b.png"));
    assert_eq!(selected(&app).len(), 3, "the selection stays while several items are renamed");
    typed(&mut app, "Trip");
    key(&mut app, Key::Enter, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    let pairs = [(r"C:\x\c", r"C:\x\Trip (1)"), (r"C:\x\a.jpg", r"C:\x\Trip (3).jpg"), (r"C:\x\b.png", r"C:\x\Trip (4).png")];
    assert_eq!(op, FileOp::MoveTo(pairs.iter().map(|&(a, b)| (p(a), p(b))).collect()));
    done(&mut app, id, op, &pairs);
    key(&mut app, Key::Char('Z'), CTRL);
    assert_eq!(op_list(&mut app), [FileOp::MoveTo(pairs.iter().map(|&(a, b)| (p(b), p(a))).collect())], "one undo step brings all names back");
}

#[test]
fn f2_and_enter_without_a_change_leave_several_selected_items_alone() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.jpg", false), ("b.png", false), ("c", true)]);
    select(&mut app, &["a.jpg", "b.png", "c"]);
    key(&mut app, Key::F(2), NO);
    assert_eq!(app.rename_text(), Some("c"));
    key(&mut app, Key::Enter, NO);
    assert!(!app.is_renaming());
    assert!(ops(&mut app).is_empty(), "an unchanged name renames nothing");
    assert_eq!(selected(&app), ["a.jpg", "b.png", "c"]);
    key(&mut app, Key::F(2), NO);
    key(&mut app, Key::Tab, NO);
    assert!(!app.is_renaming() && ops(&mut app).is_empty(), "Tab commits the same way");
}

#[test]
fn changing_the_extension_asks_yes_or_no() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("notes.txt", false), ("src", true)]);
    let rename = |app: &mut App, name: &str, text: &str| {
        select(app, &[name]);
        key(app, Key::F(2), NO);
        key(app, Key::Char('A'), CTRL);
        typed(app, text);
        key(app, Key::Enter, NO);
    };
    rename(&mut app, "notes.txt", "notes.md");
    assert!(app.is_renaming() && ops(&mut app).is_empty());
    assert_eq!(app.asking(), Some("extension"));
    assert_eq!(app.dialog_buttons(), ["Yes", "No"]);
    let (title, lines) = app.dialog_text().unwrap();
    assert_eq!((title.as_str(), lines), ("Change the file name extension?", vec!["If you change the extension of \"notes.txt\", the file might become unusable.".to_owned()]));
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.asking(), None);
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\notes.txt"), name: b"notes.md".to_vec() }], "Yes renames");
    rename(&mut app, "notes.txt", "draft.TXT");
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\notes.txt"), name: b"draft.TXT".to_vec() }]);
    rename(&mut app, "notes.txt", "notes");
    assert_eq!(app.asking(), Some("extension"), "dropping the extension asks too");
    key(&mut app, Key::Escape, NO);
    assert_eq!(app.asking(), None);
    assert!(app.is_renaming() && ops(&mut app).is_empty(), "No goes back to the name being edited");
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.asking(), Some("extension"), "submitting again asks again");
    key(&mut app, Key::Right, NO);
    key(&mut app, Key::Enter, NO);
    assert!(app.is_renaming() && ops(&mut app).is_empty());
    key(&mut app, Key::Escape, NO);
    assert!(!app.is_renaming() && ops(&mut app).is_empty());
    app.warn_extension_change = false;
    rename(&mut app, "notes.txt", "notes.log");
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\notes.txt"), name: b"notes.log".to_vec() }]);
    app.warn_extension_change = true;
    rename(&mut app, "src", "src.old");
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\src"), name: b"src.old".to_vec() }], "folders have no extension");
}

fn hint_colors(app: &mut App) -> bool {
    let error = app.look.palette.area(crate::theme::Area::Files).error.0;
    colors(app).contains(&error)
}

#[test]
fn characters_a_name_cannot_hold_are_refused_while_typing_with_a_note_by_the_editor() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::F(2), NO);
    assert!(!hint_colors(&mut app));
    typed(&mut app, "x:");
    assert_eq!(app.rename_text(), Some("x.txt"), "the colon never reaches the name");
    assert_eq!(app.rename_hint(), Some(S::UiOpsNameBadChars.t()));
    assert!(hint_colors(&mut app), "the note is drawn next to the editor");
    assert_eq!(app.tab().error, None, "nothing goes to the status bar");
    typed(&mut app, "y");
    assert_eq!((app.rename_text(), app.rename_hint()), (Some("xy.txt"), None), "the next accepted key clears the note");
    typed(&mut app, "*");
    key(&mut app, Key::Backspace, NO);
    assert_eq!((app.rename_text(), app.rename_hint()), (Some("x.txt"), None));
    key(&mut app, Key::Char('A'), CTRL);
    app.on_event(&Event::Pasted("re|port?.txt".into()), t0());
    assert_eq!(app.rename_text(), Some("report.txt"));
    assert_eq!(app.rename_hint(), Some(S::UiOpsNameBadChars.t()));
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\a.txt"), name: b"report.txt".to_vec() }]);
}

#[test]
fn a_name_taken_by_another_item_keeps_the_editor_open_with_the_typed_text() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false), ("Notes", true)]);
    let rename = |app: &mut App, text: &str| {
        key(app, Key::Char('A'), CTRL);
        typed(app, text);
        key(app, Key::Enter, NO);
    };
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::F(2), NO);
    rename(&mut app, "B.TXT");
    assert!(app.is_renaming());
    assert_eq!((app.rename_text(), app.rename_hint()), (Some("B.TXT"), Some(S::UiOpsNameTaken.t())));
    assert!(ops(&mut app).is_empty());
    rename(&mut app, "notes");
    assert_eq!((app.rename_text(), app.rename_hint()), (Some("notes"), Some(S::UiOpsNameTaken.t())), "folders count too");
    key(&mut app, Key::Tab, NO);
    assert_eq!(app.rename_text(), Some("notes"), "Tab does not move on from a taken name");
    rename(&mut app, "A.txt");
    assert!(!app.is_renaming());
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\a.txt"), name: b"A.txt".to_vec() }], "changing only the case of the same item is allowed");
}

#[test]
fn names_on_unix_folders_follow_the_file_system_of_the_mount() {
    let mut app = app_at("/home/u");
    app.caps = crate::caps::Caps { unix_paths: true };
    app.set_places(Arc::new(crate::ui::Places { drives: vec![drive_fs("/", "ext4"), drive_fs("/media/u/STICK", "vfat")], ..Default::default() }));
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::F(2), NO);
    key(&mut app, Key::Char('A'), CTRL);
    typed(&mut app, "B:/");
    assert_eq!((app.rename_text(), app.rename_hint()), (Some("B:"), Some(S::UiOpsNameSlash.t())));
    typed(&mut app, ".txt");
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p("/home/u/a.txt"), name: b"B:.txt".to_vec() }]);
    key(&mut app, Key::F(2), NO);
    key(&mut app, Key::Char('A'), CTRL);
    typed(&mut app, "B.txt");
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p("/home/u/a.txt"), name: b"B.txt".to_vec() }], "ext4 tells B.txt from b.txt");

    let mut app = app_at("/media/u/STICK/docs");
    app.caps = crate::caps::Caps { unix_paths: true };
    app.set_places(Arc::new(crate::ui::Places { drives: vec![drive_fs("/", "ext4"), drive_fs("/media/u/STICK", "vfat")], ..Default::default() }));
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::F(2), NO);
    key(&mut app, Key::Char('A'), CTRL);
    typed(&mut app, "B:");
    assert_eq!((app.rename_text(), app.rename_hint()), (Some("B"), Some(S::UiOpsNameBadChars.t())));
    typed(&mut app, ".txt");
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.rename_hint(), Some(S::UiOpsNameTaken.t()), "vfat does not tell B.txt from b.txt");
    key(&mut app, Key::Char('A'), CTRL);
    typed(&mut app, "aux.txt");
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.rename_hint(), Some(S::UiOpsNameReserved.t()));
    assert!(ops(&mut app).is_empty());
}

#[test]
fn a_rename_the_provider_refuses_reopens_the_editor_with_the_typed_name() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::F(2), NO);
    typed(&mut app, "draft");
    key(&mut app, Key::Enter, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Rename { item: p(r"C:\x\a.txt"), name: b"draft.txt".to_vec() });
    assert!(!app.is_renaming());
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::OpDone { tab, id, error: Some("Access is denied.".into()), report: None }, t0());
    assert!(app.is_renaming());
    assert_eq!((app.rename_text(), app.rename_hint()), (Some("draft.txt"), Some("Access is denied.")));
    assert_eq!(app.tab().error, None, "the message sits next to the editor instead of the status bar");
    assert_eq!(selected(&app), ["a.txt"]);
    assert!(hint_colors(&mut app));
    typed(&mut app, "final");
    assert_eq!((app.rename_text(), app.rename_hint()), (Some("final.txt"), None), "the typed base name is selected again");
    key(&mut app, Key::Enter, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Rename { item: p(r"C:\x\a.txt"), name: b"final.txt".to_vec() });

    select(&mut app, &["b.txt"]);
    key(&mut app, Key::F(2), NO);
    app.on_provider_event(ProviderEvent::OpDone { tab, id, error: Some("The disk is full.".into()), report: None }, t0());
    assert_eq!((app.rename_text(), app.rename_hint()), (Some("b.txt"), None), "an editor already open is left alone");
    assert_eq!(app.tab().error.as_deref(), Some("The disk is full."));
}

#[test]
fn a_slow_second_click_renames_and_a_double_click_still_opens() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    let start = t0();
    app.on_event(&Event::Focus(true), start);
    let at = row_xy(&app, 0);
    let click = |app: &mut App, now: Instant, clicks: u8| {
        app.on_event(&Event::MouseDown { x: at.0, y: at.1, button: MouseButton::Left, clicks }, now);
        app.on_event(&Event::MouseUp { x: at.0, y: at.1, button: MouseButton::Left }, now);
    };
    let armed = |app: &mut App| app.drain_effects().any(|e| matches!(e, Effect::Timer(d) if d == Duration::from_millis(500)));
    click(&mut app, start + Duration::from_millis(100), 1);
    assert!(!armed(&mut app), "the first click selects");
    click(&mut app, start + Duration::from_millis(200), 1);
    assert!(!armed(&mut app), "a click right after the window got focus does not rename");
    let t = start + Duration::from_secs(3);
    click(&mut app, t, 1);
    assert!(armed(&mut app));
    app.on_event(&Event::Timer, t + Duration::from_millis(100));
    assert!(!app.is_renaming());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Timer(d) if d == Duration::from_millis(400))), "an early timer is armed again");
    app.on_event(&Event::Timer, t + Duration::from_millis(500));
    assert_eq!(app.rename_text(), Some("a.txt"));
    key(&mut app, Key::Escape, NO);
    let other = (app.layout.cols[1].x + 4.0, at.1);
    let t = t + Duration::from_secs(3);
    app.on_event(&Event::MouseDown { x: other.0, y: other.1, button: MouseButton::Left, clicks: 1 }, t);
    app.on_event(&Event::MouseUp { x: other.0, y: other.1, button: MouseButton::Left }, t);
    assert!(!armed(&mut app), "a click outside the name column does not rename");

    let t = t + Duration::from_secs(3);
    click(&mut app, t, 1);
    assert!(armed(&mut app));
    click(&mut app, t + Duration::from_millis(150), 2);
    app.on_event(&Event::Timer, t + Duration::from_secs(1));
    assert!(!app.is_renaming());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path == p(r"C:\x\a.txt"))));
}

#[test]
fn the_new_item_dialog_creates_files_from_templates_folders_and_shortcuts() {
    let mut app = app_at(r"C:\x");
    with_templates(&mut app);
    deliver(&mut app, &[("New Text Document.txt", false), ("a.txt", false)]);
    key(&mut app, Key::Char('I'), CTRL_SHIFT);
    let pick = app.nav.palette.as_ref().unwrap();
    assert_eq!(pick.pick, Pick::NewItem);
    assert_eq!(pick.titles, ["Folder", "Shortcut", "Text Document", "Compressed (zipped) Folder"]);
    typed(&mut app, "text");
    key(&mut app, Key::Enter, NO);
    assert!(app.nav.palette.is_none());
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::NewFile { dir: p(r"C:\x"), name: b"New Text Document (2).txt".to_vec(), template: Template::Empty });
    done(&mut app, id, op, &[(r"C:\x\New Text Document (2).txt", r"C:\x\New Text Document (2).txt")]);
    change(&mut app, ChangeKind::Added, "New Text Document (2).txt", false);
    assert_eq!(app.rename_text(), Some("New Text Document (2).txt"));
    key(&mut app, Key::Escape, NO);
    key(&mut app, Key::Char('Z'), CTRL);
    assert_eq!(op_list(&mut app), [FileOp::Trash(vec![p(r"C:\x\New Text Document (2).txt")])]);

    key(&mut app, Key::Char('I'), CTRL_SHIFT);
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::NewFolder { dir: p(r"C:\x"), name: b"New folder".to_vec() }]);

    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Char('I'), CTRL_SHIFT);
    typed(&mut app, "shortcut");
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.nav.palette.as_ref().unwrap().pick, Pick::Shortcut(p(r"C:\x")));
    key(&mut app, Key::Enter, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Link { items: vec![p(r"C:\x\a.txt")], dest: p(r"C:\x") }, "the target starts as the selected item");
    done(&mut app, id, op, &[(r"C:\x\a.txt", r"C:\x\a.txt - Shortcut.lnk")]);
    change(&mut app, ChangeKind::Added, "a.txt - Shortcut.lnk", false);
    assert_eq!(app.rename_text(), Some("a.txt - Shortcut.lnk"));
    key(&mut app, Key::Escape, NO);

    key(&mut app, Key::Char('I'), CTRL_SHIFT);
    typed(&mut app, "shortcut");
    key(&mut app, Key::Enter, NO);
    typed(&mut app, r"relative\path");
    key(&mut app, Key::Enter, NO);
    assert!(ops(&mut app).is_empty());
    assert!(app.tab().error.as_deref().is_some_and(|e| e.contains("full path")));

    let mut app = app_at("This PC");
    key(&mut app, Key::Char('I'), CTRL_SHIFT);
    assert!(app.nav.palette.is_none());
}

#[test]
fn links_get_an_icon_of_their_own_outside_windows_and_plain_files_share_their_type_icon() {
    use crate::listing::{attr, Entry};
    use crate::ui::{icon_key, IconKind};
    let entry = |attrs| Entry { attrs, ..Entry::default() };
    let link = |name: &[u8], attrs| icon_key(name, b"txt", &entry(attrs), 16, b"/home/u", 0);
    let plain = |name: &[u8]| icon_key(name, b"txt", &entry(0), 16, b"/home/u", 0);
    assert_eq!(plain(b"a.txt"), plain(b"b.txt"));
    assert_eq!(plain(b"a.txt").1, IconKind::Type);
    assert_eq!(icon_key(b"d", b"", &entry(attr::DIR), 16, b"/home/u", 0).1, IconKind::Folder);
    if cfg!(windows) {
        assert_eq!(link(b"a.txt", attr::LINK), plain(b"a.txt"));
    } else {
        assert_eq!(link(b"a.txt", attr::LINK).1, IconKind::File);
        assert_ne!(link(b"a.txt", attr::LINK).0, link(b"b.txt", attr::LINK).0);
        assert_ne!(link(b"a.txt", attr::LINK).0, plain(b"a.txt").0);
        assert_eq!(link(b"d", attr::LINK | attr::DIR).1, IconKind::File);
    }
}

#[test]
fn built_in_templates_follow_a_language_change_and_the_users_own_keep_their_names() {
    use crate::i18n::Lang;
    let mut app = app_at(r"C:\x");
    let builtin = NewTemplate { label: "Text Document".into(), ext: ".txt".into(), stem: "New Text Document".into(), template: Template::Empty, names: Some(crate::ui::TemplateNames { label: S::MenuTextDocument, stem: S::MenuNewTextDocument }) };
    let own = NewTemplate { label: "Invoice".into(), ext: ".odt".into(), stem: "Invoice".into(), template: Template::Copy(p(r"C:\t\Invoice.odt")), names: None };
    app.set_places(Arc::new(crate::ui::Places { templates: vec![builtin, own], ..Default::default() }));
    crate::i18n::set_lang(Lang::Ru);
    key(&mut app, Key::Char('I'), CTRL_SHIFT);
    let titles = app.nav.palette.as_ref().unwrap().titles.clone();
    assert_eq!(titles[2..], [S::MenuTextDocument.in_lang(Lang::Ru).to_owned(), "Invoice".to_owned()]);
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, NO);
    let made = op_list(&mut app);
    crate::i18n::set_lang(Lang::En);
    let name = format!("{}.txt", S::MenuNewTextDocument.in_lang(Lang::Ru));
    assert_eq!(made, [FileOp::NewFile { dir: p(r"C:\x"), name: name.into_bytes(), template: Template::Empty }]);
    key(&mut app, Key::Char('I'), CTRL_SHIFT);
    assert_eq!(app.nav.palette.as_ref().unwrap().titles[2..], [S::MenuTextDocument.in_lang(Lang::En), "Invoice"], "back in English without new places");
}

#[test]
fn the_folder_icon_command_asks_for_an_icon_location_and_writes_it_for_the_selected_folder() {
    assert_eq!(icon_location(r#" "C:\Windows\System32\imageres.dll" , -113 "#), Some((p(r"C:\Windows\System32\imageres.dll"), -113)));
    assert_eq!(icon_location(r"C:\i\a,b.ico"), Some((p(r"C:\i\a,b.ico"), 0)));
    assert_eq!(icon_location(r"icons\a.ico"), None);
    use crate::listing::{attr, Entry};
    use crate::ui::{icon_key, IconKind};
    let dir = |attrs| Entry { size: 0, modified: 0, created: 0, accessed: 0, name_off: 0, attrs, name_len: 0, ext_len: 0, tags: 0 };
    let custom = |epoch| icon_key(b"photos", b"", &dir(attr::DIR | attr::READONLY), 16, br"C:\x", epoch);
    assert_eq!(custom(0).1, IconKind::CustomFolder);
    assert_ne!(custom(0).0, custom(1).0);
    assert_eq!(icon_key(b"photos", b"", &dir(attr::DIR), 16, br"C:\x", 0).1, IconKind::Folder);
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("photos", true), ("a.txt", false)]);
    select(&mut app, &["photos"]);
    app.run_action("file.folder_icon", t0());
    assert_eq!(app.nav.palette.as_ref().unwrap().pick, Pick::FolderIcon(p(r"C:\x\photos")));
    typed(&mut app, r"C:\i\cam.ico,2");
    key(&mut app, Key::Enter, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::FolderIcon { folder: p(r"C:\x\photos"), icon: Some(p(r"C:\i\cam.ico")), index: 2 });
    let epoch = app.icon_epoch;
    done(&mut app, id, op, &[(r"C:\x\photos", r"C:\i\cam.ico")]);
    assert_eq!((app.icon_epoch, app.tab().error.as_deref()), (epoch + 1, Some(r"Changed the icon of C:\x\photos")));
    select(&mut app, &["a.txt"]);
    app.run_action("file.folder_icon", t0());
    assert_eq!(app.nav.palette.as_ref().unwrap().pick, Pick::FolderIcon(p(r"C:\x")), "without a selected folder the current one changes");
}

#[test]
fn restoring_the_default_folder_icon_writes_through_the_same_operation_without_an_icon() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("photos", true)]);
    select(&mut app, &["photos"]);
    app.run_action("file.folder_icon_reset", t0());
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::FolderIcon { folder: p(r"C:\x\photos"), icon: None, index: 0 });
    let epoch = app.icon_epoch;
    done(&mut app, id, op, &[(r"C:\x\photos", r"C:\x\photos")]);
    assert_eq!((app.icon_epoch, app.tab().error.as_deref()), (epoch + 1, Some(r"Restored the default icon of C:\x\photos")));
}

#[test]
fn a_new_folder_with_the_selection_takes_the_items_and_is_renamed() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("New folder", true), ("a.txt", false), ("b.txt", false)]);
    assert!(app.run_action("file.new_folder_with_selection", t0()));
    assert!(ops(&mut app).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some("Select the items to put into a new folder"));
    select(&mut app, &["a.txt", "b.txt"]);
    app.run_action("file.new_folder_with_selection", t0());
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::NewFolder { dir: p(r"C:\x"), name: b"New folder (2)".to_vec() });
    done(&mut app, id, op, &[(r"C:\x\New folder (2)", r"C:\x\New folder (2)")]);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Move { items: vec![p(r"C:\x\a.txt"), p(r"C:\x\b.txt")], dest: p(r"C:\x\New folder (2)") });
    change(&mut app, ChangeKind::Added, "New folder (2)", true);
    assert!(!app.is_renaming(), "the folder is renamed once the items are inside");
    done(&mut app, id, op, &[(r"C:\x\a.txt", r"C:\x\New folder (2)\a.txt"), (r"C:\x\b.txt", r"C:\x\New folder (2)\b.txt")]);
    change(&mut app, ChangeKind::Removed, "a.txt", false);
    assert_eq!(app.rename_text(), Some("New folder (2)"));
    assert_eq!(app.undo_depth(), (2, 0));
    key(&mut app, Key::Escape, NO);
    key(&mut app, Key::Char('Z'), CTRL);
    assert_eq!(op_list(&mut app), [FileOp::MoveTo(vec![(p(r"C:\x\New folder (2)\a.txt"), p(r"C:\x\a.txt")), (p(r"C:\x\New folder (2)\b.txt"), p(r"C:\x\b.txt"))])]);
}

#[test]
fn a_new_stream_asks_for_its_name_and_reports_the_result() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    app.run_action("file.new_stream", t0());
    assert!(app.nav.palette.is_none());
    assert_eq!(app.tab().error.as_deref(), Some("Select a file to add a stream to"));
    select(&mut app, &["a.txt"]);
    app.run_action("file.new_stream", t0());
    assert_eq!(app.nav.palette.as_ref().unwrap().pick, Pick::Stream(p(r"C:\x\a.txt")));
    typed(&mut app, "bad:name");
    key(&mut app, Key::Enter, NO);
    assert!(ops(&mut app).is_empty() && app.nav.palette.is_none());
    app.run_action("file.new_stream", t0());
    typed(&mut app, "meta");
    key(&mut app, Key::Enter, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::NewStream { item: p(r"C:\x\a.txt"), name: b"meta".to_vec() });
    done(&mut app, id, op, &[(r"C:\x\a.txt", r"C:\x\a.txt")]);
    assert_eq!(app.tab().error.as_deref(), Some(r"Created the stream C:\x\a.txt:meta"));
    assert_eq!(app.undo_depth(), (0, 0));
}

#[test]
fn paste_into_the_selected_folder_and_copy_paths_in_quotes() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("sub", true), ("a.txt", false)]);
    for names in [&[][..], &["a.txt"], &["sub", "a.txt"]] {
        select(&mut app, names);
        key(&mut app, Key::Char('V'), CTRL_SHIFT);
        assert!(ops(&mut app).is_empty());
        assert_eq!(app.tab().error.as_deref(), Some("Select one folder to paste into"));
    }
    select(&mut app, &["sub"]);
    key(&mut app, Key::Char('V'), CTRL_SHIFT);
    assert_eq!(op_list(&mut app), [FileOp::Paste { dest: p(r"C:\x\sub") }]);
    key(&mut app, Key::Char('C'), CTRL_SHIFT);
    assert_eq!(copied(&mut app).as_deref(), Some(r#""C:\x\sub""#));
    select(&mut app, &["sub", "a.txt"]);
    key(&mut app, Key::Char('C'), CTRL_SHIFT);
    assert_eq!(copied(&mut app).as_deref(), Some("\"C:\\x\\sub\"\r\n\"C:\\x\\a.txt\""));
    select(&mut app, &[]);
    key(&mut app, Key::Char('C'), CTRL_SHIFT);
    assert_eq!(copied(&mut app).as_deref(), Some(r#""C:\x""#), "with nothing selected the folder itself is copied");
    key(&mut app, Key::Char('C'), CTRL_SHIFT);
    assert_eq!(copied(&mut app).as_deref(), Some(r#""C:\x""#), "Copy as path quotes like File Explorer");
}

#[test]
fn ctrl_alt_c_stays_free_on_windows_so_the_font_commands_can_use_ctrl_alt() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Char('C'), CTRL_ALT);
    assert!(copied(&mut app).is_none(), "Ctrl+Alt+C is not bound on Windows");
}


#[test]
fn cut_items_stay_dimmed_until_the_clipboard_changes() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Char('X'), CTRL);
    assert!(app.is_cut(b"a.txt") && !app.is_cut(b"b.txt"));
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    let dim = app.theme().selection_text.alpha(0x99).0;
    assert!(g.dl.instances.iter().any(|i| i.color == dim), "a cut item still selected after Ctrl+X is dimmed too");
    app.on_event(&Event::ClipboardChanged(7), t0());
    app.on_event(&Event::ClipboardChanged(7), t0());
    assert!(app.is_cut(b"a.txt"), "the first change is the cut itself");
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    app.on_event(&Event::ClipboardChanged(8), t0());
    assert!(!app.is_cut(b"a.txt"));
    key(&mut app, Key::Char('X'), CTRL);
    key(&mut app, Key::Char('C'), CTRL);
    assert!(!app.is_cut(b"a.txt"), "copying replaces the cut");
}

#[test]
fn the_terminal_opens_the_folder_or_up_to_five_selected_folders() {
    let mut app = app_at(r"C:\x");
    let dirs = ["d1", "d2", "d3", "d4", "d5", "d6", "d7"];
    let mut items: Vec<(&str, bool)> = dirs.iter().map(|&d| (d, true)).collect();
    items.push(("a.txt", false));
    deliver(&mut app, &items);
    app.run_action("app.terminal", t0());
    assert_eq!(launches(&mut app), [Launch::Terminal { dirs: vec![p(r"C:\x")], admin: false }]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Char('`'), CTRL_SHIFT);
    assert_eq!(launches(&mut app), [Launch::Terminal { dirs: vec![p(r"C:\x")], admin: true }]);
    let mut all = dirs.to_vec();
    all.push("a.txt");
    select(&mut app, &all);
    app.run_action("app.terminal", t0());
    let first: Vec<FsPath> = dirs[..5].iter().map(|d| p(&format!(r"C:\x\{d}"))).collect();
    assert_eq!(launches(&mut app), [Launch::Terminal { dirs: first, admin: false }]);
    assert_eq!(app.tab().error.as_deref(), Some("Opened the first 5 of 7 folders in the terminal"));
    let mut app = app_at("This PC");
    deliver(&mut app, &[("C:", true)]);
    app.run_action("app.terminal", t0());
    assert!(launches(&mut app).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some("This location cannot be opened in a terminal"), "without a known home folder there is nowhere to go");
    std::sync::Arc::make_mut(&mut app.nav.sidebar.places).folders.push((crate::ui::sidebar::KnownFolder::Home, Location::parse(r"C:\Users\u").unwrap()));
    app.run_action("app.terminal", t0());
    assert_eq!(launches(&mut app), [Launch::Terminal { dirs: vec![p(r"C:\Users\u")], admin: false }], "This PC opens the terminal in the home folder as Explorer does");
    app.navigate(Location::home_page(), t0());
    app.run_action("app.terminal", t0());
    assert_eq!(launches(&mut app), [Launch::Terminal { dirs: vec![p(r"C:\Users\u")], admin: false }], "so does the Home page");
}

#[test]
fn files_dropped_on_a_program_are_passed_to_it() {
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("run.CMD", false), ("tool.exe", false)]);
    let all = COPY | MOVE | LINK;
    let enter = |app: &mut App, paths: Vec<FsPath>, at: (f32, f32)| app.drag(&DragEvent::Enter { paths, x: at.0, y: at.1, mods: NO, allowed: all, right: false }, t0());
    let (tool, cmd, txt) = (row_xy(&app, 2), row_xy(&app, 1), row_xy(&app, 0));
    let files = vec![p(r"D:\in\f.txt"), p(r"D:\in\g.txt")];
    assert_eq!(enter(&mut app, files.clone(), tool), COPY);
    assert_eq!(app.drop_spot(), Some(Spot::Row(2)));
    assert_eq!(app.drag(&DragEvent::Drop { x: tool.0, y: tool.1, mods: NO, allowed: all }, t0()), COPY);
    assert_eq!(launches(&mut app), [Launch::With { program: p(r"C:\x\tool.exe"), args: files.clone() }]);
    assert!(ops(&mut app).is_empty());
    assert_eq!(enter(&mut app, files.clone(), cmd), COPY, "scripts take dropped files too");
    app.drag(&DragEvent::Leave, t0());
    assert_eq!(enter(&mut app, vec![p(r"C:\x\tool.exe")], tool), NONE, "a program is not dropped on itself");
    app.drag(&DragEvent::Leave, t0());
    assert_eq!(enter(&mut app, files, txt), COPY);
    assert_eq!(app.drop_spot(), Some(Spot::List), "other files still drop into the folder");
}

fn deliver_attrs(app: &mut App, items: &[(&str, u32)]) {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, attrs) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs, size: 1, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
}

#[test]
fn on_linux_files_dropped_on_an_executable_or_a_desktop_launcher_are_passed_to_it() {
    use drop_effect::*;
    let mut app = app_at("/home/u");
    app.caps = crate::caps::Caps { unix_paths: true };
    deliver_attrs(&mut app, &[("notes.txt", 0), ("plain.sh", 0), ("run.sh", attr::EXEC), ("setup.exe", 0), ("tool.AppImage", attr::EXEC), ("viewer.desktop", 0)]);
    let all = COPY | MOVE | LINK;
    let files = vec![p("/tmp/in/f.txt"), p("/tmp/in/g.txt")];
    for (row, program) in [(2, "/home/u/run.sh"), (4, "/home/u/tool.AppImage"), (5, "/home/u/viewer.desktop")] {
        let at = row_xy(&app, row);
        assert_eq!(app.drag(&DragEvent::Enter { paths: files.clone(), x: at.0, y: at.1, mods: NO, allowed: all, right: false }, t0()), COPY);
        assert_eq!(app.drop_spot(), Some(Spot::Row(row)), "{program}");
        app.drag(&DragEvent::Drop { x: at.0, y: at.1, mods: NO, allowed: all }, t0());
        assert_eq!(launches(&mut app), [Launch::With { program: p(program), args: files.clone() }]);
    }
    for row in [0, 1, 3] {
        let at = row_xy(&app, row);
        app.drag(&DragEvent::Enter { paths: files.clone(), x: at.0, y: at.1, mods: NO, allowed: all, right: false }, t0());
        assert_eq!(app.drop_spot(), Some(Spot::List), "row {row} is not a program on Linux");
        app.drag(&DragEvent::Leave, t0());
    }
}

#[test]
fn on_windows_an_executable_bit_or_a_desktop_file_does_not_take_dropped_files() {
    let mut app = app_at(r"C:\x");
    deliver_attrs(&mut app, &[("tool", attr::EXEC), ("viewer.desktop", 0)]);
    let all = drop_effect::COPY | drop_effect::MOVE | drop_effect::LINK;
    for row in [0, 1] {
        let at = row_xy(&app, row);
        app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: at.0, y: at.1, mods: NO, allowed: all, right: false }, t0());
        assert_eq!(app.drop_spot(), Some(Spot::List));
        app.drag(&DragEvent::Leave, t0());
    }
}

#[test]
fn a_shortcut_to_a_folder_opens_in_flit() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("docs.lnk", false)]);
    select(&mut app, &["docs.lnk"]);
    key(&mut app, Key::Enter, NO);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path == p(r"C:\x\docs.lnk"))));
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::Navigate { tab: tab + 9, path: p(r"E:\") }, t0());
    assert_eq!(app.current_location(), &Location::parse(r"C:\x").unwrap());
    app.on_provider_event(ProviderEvent::Navigate { tab, path: p(r"D:\docs") }, t0());
    assert_eq!(app.current_location(), &Location::parse(r"D:\docs").unwrap());
}

#[test]
fn restore_all_from_the_sidebar_lists_the_bin_first_when_it_is_not_open() {
    let mut app = app_at(r"C:\work");
    app.drain_effects().for_each(drop);
    assert!(app.run_action("bin.restore_all", t0()));
    let (gen, out, path) = app
        .drain_effects()
        .find_map(|e| if let Effect::Provider { req: ProviderRequest::List { gen, out, path, .. }, .. } = e { Some((gen, out, path)) } else { None })
        .expect("the bin is listed");
    assert_eq!(path, *Location::recycle_bin().path());
    assert!(app.tab().error.is_none());
    let (listing, keys) = bin_listing(&[BIN_REPORT, BIN_OLD]);
    let snap = |done: bool, listing: Listing, keys: SortKeys| Arc::new(DirSnapshot { gen, path: path.clone(), order: (0..listing.len() as u32).collect(), listing, keys, view: ViewSpec::default(), done, error: None, elapsed: Duration::ZERO });
    out.publish(snap(false, Listing::default(), SortKeys::new(CaseMode::Insensitive)));
    app.poll();
    assert!(op_list(&mut app).is_empty(), "a partial listing restores nothing");
    out.publish(snap(true, listing, keys));
    app.poll();
    assert_eq!(app.asking(), Some("confirm"));
    assert!(op_list(&mut app).is_empty(), "the bin is read first, then the restore is confirmed");
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    assert_eq!(app.dialog_buttons(), ["Restore", "Cancel"]);
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::Restore(pairs(&[BIN_REPORT, BIN_OLD]))]);
    app.poll();
    assert!(op_list(&mut app).is_empty());
    app.run_action("bin.restore_all", t0());
    let (gen, out) = app.drain_effects().find_map(|e| if let Effect::Provider { req: ProviderRequest::List { gen, out, .. }, .. } = e { Some((gen, out)) } else { None }).unwrap();
    out.publish(Arc::new(DirSnapshot { gen, path: path.clone(), listing: Listing::default(), keys: SortKeys::new(CaseMode::Insensitive), order: Vec::new(), view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO }));
    app.poll();
    assert_eq!(app.tab().error.as_deref(), Some(S::UiEmptyBin.t()));
    app.run_action("bin.restore", t0());
    assert_eq!(app.tab().error.as_deref(), Some("Open the Recycle Bin to restore items"));
}

#[test]
fn pasted_text_arrives_as_an_event() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::F(2), NO);
    key(&mut app, Key::Char('A'), CTRL);
    app.on_event(&Event::Pasted("b.txt\nsecond line".into()), t0());
    assert_eq!(app.rename_text(), Some("b.txt"));
}

#[test]
fn typed_paths_follow_the_platform_caps() {
    let mut app = app_at("/home/u");
    deliver(&mut app, &[("a.txt", false)]);
    select(&mut app, &["a.txt"]);
    app.submit_pick(Pick::Shortcut(p("/home/u")), "/etc/hosts");
    assert!(ops(&mut app).is_empty(), "Windows caps refuse a Unix target");
    app.caps = crate::caps::Caps { unix_paths: true };
    app.submit_pick(Pick::Shortcut(p("/home/u")), " /etc/hosts ");
    let [(_, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Link { items: vec![p("/etc/hosts")], dest: p("/home/u") });
    app.submit_pick(Pick::Shortcut(p("/home/u")), r"C:\x");
    assert!(ops(&mut app).is_empty(), "Unix caps refuse a drive path");
}

fn bulk(app: &mut App, n: usize) -> (Vec<String>, OpId, FileOp) {
    let names: Vec<String> = (0..n).map(|k| format!("IMG_{k:04}.jpg")).collect();
    let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), false)).chain([("keep.txt", false)]).collect();
    deliver(app, &items);
    let all: Vec<&str> = names.iter().map(String::as_str).collect();
    select(app, &all);
    key(app, Key::Delete, NO);
    let [(id, op)] = ops(app).try_into().unwrap();
    (names, id, op)
}

fn watcher_removes(app: &mut App, names: &[String]) {
    let (tab, gen) = (app.tab().id, app.tab().gen);
    for n in names {
        let change = DirChange { kind: ChangeKind::Removed, name: n.as_bytes().to_vec(), meta: None };
        app.on_provider_event(ProviderEvent::Changes { tab, gen, changes: vec![change] }, t0());
    }
    app.poll();
}

fn shown(app: &App) -> String {
    let v = app.tab().view.as_ref().unwrap();
    match v.rows() {
        [one] => String::from_utf8_lossy(v.listing().name(*one)).into_owned(),
        rows => format!("{} rows", rows.len()),
    }
}

#[test]
fn removals_from_a_bulk_trash_land_in_one_pass_without_waiting_for_frames() {
    let mut app = app_at(r"C:\pics");
    let (names, _, op) = bulk(&mut app, 1250);
    assert!(matches!(&op, FileOp::Trash(p) if p.len() == 1250));
    let started = Instant::now();
    watcher_removes(&mut app, &names);
    let spent = started.elapsed();
    assert_eq!(shown(&app), "keep.txt", "every confirmed removal is applied as soon as the events are polled");
    assert!(app.tab().pending_diffs.is_empty());
    assert!(spent < Duration::from_millis(500), "1250 single removals took {spent:?}");
    let mut g = Gfx::new(Mono, 1024);
    assert!(!app.frame(&mut g, t0()), "nothing is left to animate");
}

#[test]
fn a_context_menu_during_a_bulk_trash_still_ends_with_the_deleted_rows_gone() {
    let mut app = app_at(r"C:\pics");
    let (names, id, op) = bulk(&mut app, 1250);
    let mut g = Gfx::new(Mono, 1024);
    watcher_removes(&mut app, &names[..400]);
    app.frame(&mut g, t0());
    let spot = row_xy(&app, 0);
    press(&mut app, spot, MouseButton::Right, NO);
    release(&mut app, spot, MouseButton::Right);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::ShellMenu(_))));
    watcher_removes(&mut app, &names[400..900]);
    let report: Vec<(String, String)> = names.iter().map(|n| (format!(r"C:\pics\{n}"), format!(r"C:\$Recycle.Bin\S-1\$R{n}"))).collect();
    let pairs: Vec<(&str, &str)> = report.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    done(&mut app, id, op, &pairs);
    app.poll();
    assert_eq!(shown(&app), "keep.txt", "rows the operation reports as gone leave even while the shell menu holds the frames");
    watcher_removes(&mut app, &names[900..]);
    app.frame(&mut g, t0());
    assert_eq!(shown(&app), "keep.txt");
}

fn focused(app: &App) -> Option<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.focus.map(|f| String::from_utf8_lossy(v.listing().name(f)).into_owned())
}

fn trash(app: &mut App, names: &[&str]) {
    select(app, names);
    key(app, Key::Delete, NO);
    let [(id, op)] = ops(app).try_into().unwrap();
    let pairs: Vec<(String, String)> = names.iter().map(|n| (format!(r"C:\x\{n}"), format!(r"C:\$Recycle.Bin\S\$R{n}"))).collect();
    done(app, id, op, &pairs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect::<Vec<_>>());
}

#[test]
fn after_a_delete_the_item_in_its_place_takes_focus_and_selection() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a", false), ("b", false), ("c", false), ("d", false), ("e", false), ("f", false), ("g", false)]);
    trash(&mut app, &["c", "b"]);
    assert_eq!((selected(&app), focused(&app)), (vec!["d".to_owned()], Some("d".into())));
    key(&mut app, Key::Down, NO);
    assert_eq!(selected(&app), ["e"], "the arrow keys go on from there");
    trash(&mut app, &["g"]);
    assert_eq!((selected(&app), focused(&app)), (vec!["f".to_owned()], Some("f".into())), "removing the last item selects the one before it");

    app.tab_mut().view.as_mut().unwrap().clear_selection();
    change(&mut app, ChangeKind::Removed, "a", false);
    assert_eq!((selected(&app), focused(&app)), (Vec::<String>::new(), Some("f".into())), "an unrelated item that vanishes changes nothing");
    change(&mut app, ChangeKind::Removed, "f", false);
    assert_eq!((selected(&app), focused(&app)), (Vec::<String>::new(), Some("e".into())), "focus moves on but nothing gets selected for the user");
    select(&mut app, &["d", "e"]);
    change(&mut app, ChangeKind::Removed, "d", false);
    assert_eq!((selected(&app), focused(&app)), (vec!["e".to_owned()], Some("e".into())), "the rest of the selection stays as it was");
}

#[test]
fn the_item_that_takes_the_place_of_a_deleted_one_is_scrolled_into_view() {
    let mut app = app_at(r"C:\x");
    let names: Vec<String> = (0..200).map(|k| format!("f{k:03}")).collect();
    let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), false)).collect();
    deliver(&mut app, &items);
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    assert_eq!(app.tab().scroll_target, 0.0);
    trash(&mut app, &["f150"]);
    app.frame(&mut g, t0());
    assert_eq!(selected(&app), ["f151"]);
    let t = app.tab();
    let row = t.view.as_ref().unwrap().row_of(t.view.as_ref().unwrap().focus.unwrap()).unwrap() as f32;
    let top = row * app.layout.row_h;
    assert!(t.scroll_target <= top && top + app.layout.row_h <= t.scroll_target + app.layout.list.h, "row {row} at {top} is visible from {}", t.scroll_target);
}

#[test]
fn item_paths_of_a_plain_folder_match_the_general_path_for_every_kind_of_folder() {
    for dir in [r"C:\Users\me", r"C:\", r"\\server\share\docs", r"\\?\C:\deep"] {
        let mut app = app_at(dir);
        deliver(&mut app, &[("a.txt", false), ("sub", true), ("Ubuntu", true)]);
        let items: Vec<u32> = (0..3).collect();
        let general: Option<Vec<FsPath>> = items.iter().map(|&i| app.tab().entry_location(i).to_windows()).collect();
        assert_eq!(app.item_paths(&items).ok(), general, "{dir}");
    }
}

#[test]
fn iso_dates_follow_the_utc_offset() {
    let ticks = 133_486_272_000_000_000;
    assert_eq!(iso_date(ticks, 0), "2024-01-02");
    assert_eq!(iso_date(ticks, -60), "2024-01-01");
}

#[test]
fn rename_inserts_ids_dates_and_changes_case() {
    let mut app = app_at(r"C:\x");
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    let i = listing.push(b"photo.jpg", Meta { size: 1, modified: 133_486_272_000_000_000, created: 0, ..Default::default() });
    keys.push(listing.name(i));
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    assert!(!app.available("rename.insert_id") && !app.available("text.uppercase"));
    select(&mut app, &["photo.jpg"]);
    key(&mut app, Key::F(2), NO);
    assert!(app.available("rename.insert_id") && app.available("text.uppercase"));
    assert!(app.run_action("rename.insert_id", t0()));
    typed(&mut app, "_");
    assert!(app.run_action("rename.insert_id", t0()));
    assert_eq!(app.rename_text(), Some("1_2.jpg"));
    typed(&mut app, "_");
    assert!(app.run_action("rename.insert_modified", t0()));
    assert_eq!(app.rename_text(), Some("1_2_2024-01-02.jpg"));
    assert!(app.run_action("rename.insert_created", t0()));
    assert_eq!(app.rename_text(), Some("1_2_2024-01-02.jpg"), "an unknown creation date inserts nothing");
    key(&mut app, Key::Char('A'), CTRL);
    assert!(app.run_action("text.uppercase", t0()));
    assert_eq!(app.rename_text(), Some("1_2_2024-01-02.JPG"));
    assert!(app.run_action("text.lowercase", t0()));
    assert_eq!(app.rename_text(), Some("1_2_2024-01-02.jpg"));
    key(&mut app, Key::Escape, NO);
    key(&mut app, Key::F(2), NO);
    assert!(app.run_action("rename.insert_id", t0()));
    assert_eq!(app.rename_text(), Some("1.jpg"), "the counter restarts with every rename");
}

#[test]
fn paste_shortcut_asks_for_links_in_the_current_folder() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    assert!(app.run_action("file.paste_shortcut", t0()));
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::PasteLink { dest: p(r"C:\x") }]);
}

fn colors(app: &mut App) -> Vec<u32> {
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    g.dl.instances.iter().map(|i| i.color).collect()
}

#[test]
fn rename_dims_other_rows_and_selects_on_the_accent() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false), ("c.txt", false)]);
    let files = *app.look.palette.area(crate::theme::Area::Files);
    let veil = files.bg.alpha(RENAME_VEIL).0;
    assert!(!colors(&mut app).contains(&veil));
    select(&mut app, &["b.txt"]);
    key(&mut app, Key::F(2), NO);
    let drawn = colors(&mut app);
    assert_eq!(drawn.iter().filter(|&&c| c == veil).count(), 2, "rows above and below are veiled");
    assert!(drawn.contains(&files.accent.0), "the selected base name sits on the accent");
}

#[test]
fn a_file_drop_on_the_panel_fills_it_with_teal_and_a_folder_lights_only_its_row() {
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("sub", true), ("a.txt", false)]);
    let files = *app.look.palette.area(crate::theme::Area::Files);
    let fill = files.teal.alpha(DROP_FILL).0;
    let empty = (300.0, app.layout.list.y + 200.0);
    app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: empty.0, y: empty.1, mods: NO, allowed: COPY | MOVE, right: false }, t0());
    assert!(colors(&mut app).contains(&fill));
    let sub = row_xy(&app, 0);
    app.drag(&DragEvent::Over { x: sub.0, y: sub.1, mods: NO, allowed: COPY | MOVE }, t0());
    let drawn = colors(&mut app);
    assert!(!drawn.contains(&fill) && drawn.contains(&files.selection.0));
    app.drag(&DragEvent::Leave, t0());
}

#[test]
fn the_rename_menu_opens_with_the_mouse_or_the_menu_key() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::F(2), NO);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::F(10), SHIFT);
    let labels: Vec<String> = app
        .drain_effects()
        .find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None })
        .unwrap()
        .into_iter()
        .filter_map(|i| if let MenuItem::Command { label, .. } = i { Some(label) } else { None })
        .collect();
    assert_eq!(labels, ["Undo", "Cut", "Copy", "Paste", "Delete", "Select All", "Insert Incremental ID", "Insert Current Date", "Insert File Create Date", "Insert File Last Modified Date", "Uppercase", "Lowercase"]);
    assert!(app.is_renaming());
}

#[test]
fn an_error_in_the_status_bar_goes_away_after_five_seconds() {
    let mut a = crate::ui::harness::app(&["a.txt"]);
    let mut g = Gfx::new(crate::text::tests::Mono, 256);
    let t0 = Instant::now();
    a.status("Something failed");
    a.frame(&mut g, t0);
    a.on_event(&Event::Timer, t0 + ERROR_FOR - Duration::from_millis(1));
    assert!(a.tab().error.is_some(), "it stays for the full five seconds");
    a.status("Something else failed");
    a.frame(&mut g, t0 + ERROR_FOR - Duration::from_millis(1));
    a.on_event(&Event::Timer, t0 + ERROR_FOR);
    assert!(a.tab().error.is_some(), "a new error starts its own five seconds");
    a.on_event(&Event::Timer, t0 + ERROR_FOR * 2);
    assert!(a.tab().error.is_none());
    assert!(a.dirty);
}

#[test]
fn single_click_opens_a_folder_on_release_so_it_can_still_be_dragged() {
    let mut app = app_at(r"C:\x");
    app.single_click_folders = crate::ui::settings::ClickOpen::Always;
    deliver(&mut app, &[("a", true), ("b", true)]);
    let (x, y) = row_xy(&app, 0);
    press(&mut app, (x, y), MouseButton::Left, NO);
    assert_eq!(app.tab().location().path(), &p(r"C:\x"), "the press only selects");
    assert_eq!(selected(&app), ["a"]);
    app.on_event(&Event::MouseMove { x: x + 12.0, y: y + 3.0 }, t0());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::StartDrag(p) if p.len() == 1)));
    release(&mut app, (x + 12.0, y + 3.0), MouseButton::Left);
    assert_eq!(app.tab().location().path(), &p(r"C:\x"), "a drag never opens the folder");
    let b = row_xy(&app, 1);
    press(&mut app, b, MouseButton::Left, NO);
    assert_eq!(app.tab().location().path(), &p(r"C:\x"));
    release(&mut app, b, MouseButton::Left);
    assert_eq!(app.tab().location().path(), &p(r"C:\x\b"), "the release opens it");
}

#[test]
fn double_clicking_the_right_edge_of_a_column_fits_that_column() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    let l = &app.layout;
    let i = (1..l.cols.len()).find(|&i| l.columns[i] != crate::ui::Column::Name).expect("a column besides the name");
    let (col, x, y) = (l.columns[i], l.cols[i].right(), l.header.y + l.header.h / 2.0);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t0());
    release(&mut app, (x, y), MouseButton::Left);
    assert_eq!(app.tab().fit_column, None);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 2 }, t0());
    release(&mut app, (x, y), MouseButton::Left);
    assert_eq!(app.tab().fit_column, Some(col));
    assert!(app.tab().fit_pending);
    assert!(matches!(app.drag, Drag::None));
}

#[test]
fn run_in_terminal_launches_only_an_executable_file() {
    let mut app = app_at("/home/u");
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for (n, attrs) in [("notes.txt", 0), ("tool", attr::EXEC)] {
        let i = listing.push(n.as_bytes(), Meta { attrs, size: 1, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    let launched = |app: &mut App| app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Launch { launch: Launch::RunInTerminal { program }, .. }, .. } => Some(program),
        _ => None,
    });
    select(&mut app, &["notes.txt"]);
    app.run_in_terminal();
    assert_eq!(launched(&mut app), None);
    select(&mut app, &["tool"]);
    app.run_in_terminal();
    assert_eq!(launched(&mut app), Some(p("/home/u/tool")));
}

#[test]
fn the_rename_box_picks_words_and_cuts_with_shift_delete_without_deleting_files() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("my report.txt", false), ("other.txt", false)]);
    select(&mut app, &["my report.txt"]);
    key(&mut app, Key::F(2), NO);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Delete, SHIFT);
    let effects: Vec<Effect> = app.drain_effects().collect();
    assert!(effects.iter().any(|e| matches!(e, Effect::SetClipboard(s) if s == "my report")));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { .. }, .. })));
    assert!(!app.jobs.modal());
    assert_eq!(app.rename_text(), Some(".txt"));
    key(&mut app, Key::Char('Z'), CTRL);
    assert_eq!(app.rename_text(), Some("my report.txt"));
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    let r = app.ops.rename.as_ref().unwrap().rect;
    let at = (r.x + 5.0, r.y + r.h / 2.0);
    let t = t0();
    let mut click = |app: &mut App, clicks: u8, now: Instant| {
        app.on_event(&Event::MouseDown { x: at.0, y: at.1, button: MouseButton::Left, clicks }, now);
        app.on_event(&Event::MouseUp { x: at.0, y: at.1, button: MouseButton::Left }, now);
        app.frame(&mut g, now);
        let e = &app.ops.rename.as_ref().unwrap().edit;
        e.text[e.selection()].to_owned()
    };
    assert_eq!(click(&mut app, 1, t), "");
    assert_eq!(click(&mut app, 2, t), "my");
    assert_eq!(click(&mut app, 1, t + Duration::from_millis(50)), "my report.txt");
    key(&mut app, Key::Insert, CTRL);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::SetClipboard(s) if s == "my report.txt")));
    assert!(app.is_renaming());
}

#[test]
fn copied_paths_are_separated_by_the_line_break_of_the_system() {
    let mut app = app_at("/home/u");
    app.caps = crate::caps::Caps { unix_paths: true };
    deliver(&mut app, &[("a b.txt", false), ("c.txt", false)]);
    select(&mut app, &["a b.txt", "c.txt"]);
    app.drain_effects().for_each(drop);
    assert!(app.run_action("item.copy_path", t0()));
    assert_eq!(copied(&mut app).as_deref(), Some("/home/u/a b.txt\n/home/u/c.txt"));
    assert!(app.run_action("item.copy_quoted_path", t0()));
    assert_eq!(copied(&mut app).as_deref(), Some("\"/home/u/a b.txt\"\n\"/home/u/c.txt\""));
}

fn this_pc_with_drives(names: &[&str]) -> App {
    let mut app = App::new(Persisted { path: Location::computer(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), t0());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in names {
        let i = listing.push(n.as_bytes(), Meta { attrs: attr::DIR | attr::VOLUME, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    let drives = vec![
        Drive { label: "System (C:)".into(), ..drive_fs(r"C:\", "NTFS") },
        Drive { label: "USB Drive (E:)".into(), kind: Some(S::SidebarDriveUsb), removable: true, ..drive_fs(r"E:\", "FAT32") },
        Drive { label: "docs (nas) (Z:)".into(), remote: Some(r"\\nas\docs".into()), ..drive_fs(r"Z:\", "") },
    ];
    app.nav.sidebar.places = Arc::new(super::super::sidebar::Places { drives, ..Default::default() });
    app
}

fn labels(app: &mut App) -> Vec<(FsPath, String)> {
    app.drain_effects().filter_map(|e| if let Effect::SetVolumeLabel { drive, label } = e { Some((drive, label)) } else { None }).collect()
}

#[test]
fn f2_on_a_drive_in_this_pc_edits_its_volume_label() {
    let mut app = this_pc_with_drives(&["C:", "E:", "Z:", "Ubuntu"]);
    select(&mut app, &["C:"]);
    key(&mut app, Key::F(2), NO);
    assert_eq!(app.rename_text(), Some("System"), "the edit starts from the label, not the drive letter");
    key(&mut app, Key::End, NO);
    typed(&mut app, " 2");
    key(&mut app, Key::Enter, NO);
    assert_eq!(labels(&mut app), [(p(r"C:\"), "System 2".to_owned())]);
    assert!(!app.is_renaming());
    select(&mut app, &["E:"]);
    key(&mut app, Key::F(2), NO);
    assert_eq!(app.rename_text(), Some(""), "a drive without a label starts empty");
    key(&mut app, Key::Enter, NO);
    assert!(labels(&mut app).is_empty(), "an unchanged label is not written");
    for other in ["Z:", "Ubuntu"] {
        select(&mut app, &[other]);
        key(&mut app, Key::F(2), NO);
        assert!(!app.is_renaming(), "{other} has no label to edit");
    }
}

#[test]
fn renaming_a_drive_is_offered_in_its_menu_and_shell_failures_show_in_the_status_bar() {
    let mut app = this_pc_with_drives(&["C:", "Z:"]);
    select(&mut app, &["C:"]);
    assert!(app.labelled_drive().is_some());
    select(&mut app, &["Z:"]);
    assert!(app.labelled_drive().is_none());
    app.on_event(&Event::ShellError("Cannot open the Share window: Access is denied.".into()), t0());
    assert_eq!(app.tab().error.as_deref(), Some("Cannot open the Share window: Access is denied."));
}

#[test]
fn run_in_terminal_on_windows_takes_programs_and_scripts_by_extension() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("setup.exe", false), ("build.CMD", false), ("notes.txt", false), ("tools.exe", true)]);
    let launched = |app: &mut App| app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Launch { launch: Launch::RunInTerminal { program }, .. }, .. } => Some(program),
        _ => None,
    });
    for (name, expected) in [("setup.exe", Some(p(r"C:\x\setup.exe"))), ("build.CMD", Some(p(r"C:\x\build.CMD"))), ("notes.txt", None), ("tools.exe", None)] {
        select(&mut app, &[name]);
        app.run_in_terminal();
        assert_eq!(launched(&mut app), expected, "{name}");
    }
    assert!(app.caps.supports("file.run_in_terminal"));
    app.caps.unix_paths = true;
    select(&mut app, &["setup.exe"]);
    app.run_in_terminal();
    assert_eq!(launched(&mut app), None, "on Linux only the executable bit counts");
}


#[test]
fn trashing_shows_an_undo_bar_that_untrashes_and_disappears() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    select(&mut app, &["a.txt", "b.txt"]);
    key(&mut app, Key::Delete, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert!(app.tab().undo.is_none(), "nothing shows until the trash is done");
    done(&mut app, id, op.clone(), &[(r"C:\x\a.txt", r"C:\$Recycle.Bin\S\$R1"), (r"C:\x\b.txt", r"C:\$Recycle.Bin\S\$R2")]);
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    assert_eq!(app.tab().undo.map(|(n, _)| n), Some(2));
    let a11y = app.accessibility();
    assert!(a11y.nodes.iter().any(|node| node.role == crate::a11y::Role::Button && node.name == "Undo"), "accessibility sees the bar");
    let r = app.layout.bar.iter().find(|(b, _)| *b == crate::ui::bottom::BottomHit::Undo).map(|(_, r)| *r).unwrap();
    press(&mut app, (r.x + r.w / 2.0, r.y + r.h / 2.0), MouseButton::Left, NO);
    release(&mut app, (r.x + r.w / 2.0, r.y + r.h / 2.0), MouseButton::Left);
    let [(undo_id, undo_op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(undo_op, FileOp::Restore(vec![(p(r"C:\$Recycle.Bin\S\$R1"), p(r"C:\x\a.txt")), (p(r"C:\$Recycle.Bin\S\$R2"), p(r"C:\x\b.txt"))]));
    assert!(app.tab().undo.is_none(), "clicking Undo removes the bar");
    done(&mut app, undo_id, undo_op, &[(r"C:\$Recycle.Bin\S\$R1", r"C:\x\a.txt"), (r"C:\$Recycle.Bin\S\$R2", r"C:\x\b.txt")]);

    deliver(&mut app, &[("a.txt", false), ("b.txt", false), ("c.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Delete, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    done(&mut app, id, op, &[(r"C:\x\a.txt", r"C:\$Recycle.Bin\S\$R3")]);
    assert_eq!(app.tab().undo.map(|(n, _)| n), Some(1));
    select(&mut app, &["b.txt"]);
    key(&mut app, Key::Delete, NO);
    assert!(app.tab().undo.is_none(), "a new operation clears the bar");
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    done(&mut app, id, op, &[(r"C:\x\b.txt", r"C:\$Recycle.Bin\S\$R4")]);
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    assert_eq!(app.tab().undo.map(|(n, _)| n), Some(1));
    select(&mut app, &["c.txt"]);
    key(&mut app, Key::Delete, SHIFT);
    assert_eq!(app.tab().undo.map(|(n, _)| n), Some(1), "the delete is still waiting for the confirmation");
    key(&mut app, Key::Enter, NO);
    assert!(app.tab().undo.is_none(), "starting the permanent delete clears the bar");
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert!(matches!(op, FileOp::Delete(_)));
    done(&mut app, id, op, &[(r"C:\x\c.txt", r"C:\x\c.txt")]);
    assert!(app.tab().undo.is_none(), "a permanent delete shows no bar");

    if let Some(t) = app.tabs.get_mut(app.cur) { t.undo = Some((1, t0() - Duration::from_secs(6))); }
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    assert!(app.tab().undo.is_none(), "the bar expires after five seconds");
}

#[test]
fn the_undo_bar_belongs_to_its_tab_and_yields_to_status_errors() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Delete, NO);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    done(&mut app, id, op, &[(r"C:\x\a.txt", r"C:\$Recycle.Bin\S\$R1")]);
    assert_eq!(app.tab().undo.map(|(n, _)| n), Some(1));
    app.status("the folder went away");
    assert_eq!(app.tab().error.as_deref(), Some("the folder went away"), "the error stays visible while the bar waits");
    key(&mut app, Key::Char('T'), CTRL);
    deliver(&mut app, &[("b.txt", false)]);
    assert_ne!(app.cur, 0, "the second tab is active now");
    let a11y = app.accessibility();
    assert!(!a11y.nodes.iter().any(|node| node.name == "Undo"), "the bar does not follow into another tab");
    key(&mut app, Key::Tab, CTRL);
    assert_eq!(app.tab().undo.map(|(n, _)| n), Some(1), "the bar waits in its own tab");
}

#[test]
fn hovering_a_sidebar_folder_mid_drag_opens_it_after_the_spring_delay() {
    let mut app = app_at(r"C:\x");
    let downloads = crate::location::Location::parse(r"C:\Users\u\Downloads").unwrap();
    app.set_places(std::sync::Arc::new(crate::ui::Places { folders: vec![(crate::ui::KnownFolder::Home, crate::location::Location::parse(r"C:\Users\u").unwrap()), (crate::ui::KnownFolder::Downloads, downloads.clone())], recycle_bin: true, ..Default::default() }));
    app.spring_delay_ms = 100;
    let all = drop_effect::COPY | drop_effect::MOVE | drop_effect::LINK;
    let k = app.nav.sidebar.items.iter().position(|it| it.loc == downloads).unwrap();
    let r = app.nav.sidebar.rects[k];
    let at = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: at.0, y: at.1, mods: NO, allowed: all, right: false }, t0());
    app.drag(&DragEvent::Over { x: at.0, y: at.1, mods: NO, allowed: all }, t0());
    assert_eq!(*app.current_location(), crate::location::Location::parse(r"C:\x").unwrap(), "nothing before the delay");
    app.drag(&DragEvent::Over { x: at.0, y: at.1, mods: NO, allowed: all }, t0() + Duration::from_millis(150));
    assert_eq!(*app.current_location(), downloads, "the sidebar folder opens mid drag");
}

#[test]
fn dragging_at_the_list_edge_scrolls_and_the_sidebar_does_too() {
    let mut app = app_at(r"C:\x");
    let names: Vec<String> = (0..200).map(|i| format!("f{i:03}.txt")).collect();
    let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), false)).collect();
    deliver(&mut app, &items);
    assert!(app.max_scroll() > 0.0);
    let all = drop_effect::COPY | drop_effect::MOVE | drop_effect::LINK;
    let inside = (app.layout.list.x + app.layout.list.w / 2.0, app.layout.list.bottom() - 3.0);
    app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: inside.0, y: inside.1, mods: NO, allowed: all, right: false }, t0());
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    app.frame(&mut Gfx::new(Mono, 1024), t0() + Duration::from_millis(100));
    assert!(app.tab().scroll > 0.0, "the list scrolls while the drag holds the bottom edge");
    app.drag(&DragEvent::Over { x: inside.0, y: app.layout.list.y + app.layout.list.h / 2.0, mods: NO, allowed: all }, t0() + Duration::from_millis(150));
    let held = app.tab().scroll_target;
    app.frame(&mut Gfx::new(Mono, 1024), t0() + Duration::from_millis(250));
    assert_eq!(app.tab().scroll_target, held, "the middle of the list does not scroll");
    app.drag(&DragEvent::Leave, t0() + Duration::from_millis(300));
    app.frame(&mut Gfx::new(Mono, 1024), t0() + Duration::from_millis(400));
    assert_eq!(app.tab().scroll_target, held, "the scroll holds after the drag leaves");
}

#[test]
fn dragging_after_a_double_click_in_the_rename_box_goes_by_whole_words() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("my annual report.txt", false)]);
    select(&mut app, &["my annual report.txt"]);
    key(&mut app, Key::F(2), NO);
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    let r = app.ops.rename.as_ref().unwrap().rect;
    let y = r.y + r.h / 2.0;
    let t = t0();
    let mut drag = |app: &mut App, clicks: u8, from: f32, to: f32, now: Instant| {
        app.on_event(&Event::MouseDown { x: from, y, button: MouseButton::Left, clicks }, now);
        app.frame(&mut g, now);
        app.on_event(&Event::MouseMove { x: to, y }, now);
        app.frame(&mut g, now);
        app.on_event(&Event::MouseUp { x: to, y, button: MouseButton::Left }, now);
        let e = &app.ops.rename.as_ref().unwrap().edit;
        e.text[e.selection()].to_owned()
    };
    assert_eq!(drag(&mut app, 2, r.x + 5.0, r.x + 45.0, t), "my annual", "the word reached is taken whole");
    assert_eq!(drag(&mut app, 1, r.x + 5.0, r.x + 45.0, t + Duration::from_secs(2)), " ann", "a single click still drags by characters");
    assert!(app.is_renaming());
}

#[test]
fn extract_and_delete_selects_what_came_out_of_the_archive() {
    for added_first in [true, false] {
        let mut app = app_at(r"C:\x");
        deliver(&mut app, &[("a.zip", false), ("b.txt", false)]);
        select(&mut app, &["a.zip"]);
        app.extract_and_delete();
        let [(id, extract)] = ops(&mut app).try_into().unwrap();
        done(&mut app, id, extract, &[(r"C:\x\a.zip", r"C:\x\a")]);
        let [(trash_id, trash)] = ops(&mut app).try_into().unwrap();
        assert_eq!(trash, FileOp::Trash(vec![p(r"C:\x\a.zip")]));
        if added_first {
            change(&mut app, ChangeKind::Added, "a", true);
        }
        done(&mut app, trash_id, trash, &[(r"C:\x\a.zip", r"C:\$Recycle.Bin\S\$R1")]);
        if !added_first {
            change(&mut app, ChangeKind::Added, "a", true);
        }
        assert_eq!(selected(&app), ["a"], "the extracted folder is selected whichever arrives first");
        let v = app.tab().view.as_ref().unwrap();
        assert_eq!(v.focus.map(|i| v.listing().name(i).to_vec()), Some(b"a".to_vec()));
    }
}

#[test]
fn one_selected_picture_becomes_the_desktop_background() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("photo.jpg", false), ("notes.txt", false), ("more.png", false)]);
    select(&mut app, &["photo.jpg"]);
    assert!(app.run_action("file.wallpaper", t0()));
    let set: Vec<Launch> = app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect();
    assert_eq!(set, [Launch::Wallpaper(p(r"C:\x\photo.jpg"))]);
}

#[test]
fn a_document_or_several_pictures_do_not_change_the_background() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("photo.jpg", false), ("notes.txt", false), ("more.png", false)]);
    for names in [&["notes.txt"][..], &["photo.jpg", "more.png"], &[]] {
        select(&mut app, names);
        app.run_action("file.wallpaper", t0());
        assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Launch { .. }, .. })), "{names:?}");
        assert_eq!(app.tab().error.as_deref(), Some(S::OpsExtraSelectWallpaper.t()));
    }
}

#[test]
fn back_and_forward_asked_by_the_shell_walk_the_tab_history() {
    let mut app = app_at(r"C:\a");
    app.navigate(Location::parse(r"C:\b").unwrap(), t0());
    app.on_event(&Event::ShellVerb("back".into()), t0());
    assert_eq!(app.tab().location().path(), &p(r"C:\a"));
    app.on_event(&Event::ShellVerb("Forward".into()), t0());
    assert_eq!(app.tab().location().path(), &p(r"C:\b"));
    app.on_event(&Event::ShellVerb("forward".into()), t0());
    assert_eq!(app.tab().location().path(), &p(r"C:\b"), "nothing ahead leaves the tab where it is");
}

#[test]
fn hovering_a_plain_file_hands_the_drop_to_its_shell_handler_with_flit_as_the_fallback() {
    use drop_effect::*;
    let all = COPY | MOVE | LINK;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("sub", true), ("go.lnk", false), ("run.exe", false), ("script.py", false)]);
    let row = |app: &App, name: &str| {
        let v = app.tab().view.as_ref().unwrap();
        row_xy(app, v.rows().iter().position(|&i| v.listing().name(i) == name.as_bytes()).unwrap())
    };
    let enter = |app: &mut App, at: (f32, f32)| app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: at.0, y: at.1, mods: NO, allowed: all, right: false }, t0());
    for name in ["go.lnk", "script.py"] {
        let at = row(&app, name);
        assert_eq!(enter(&mut app, at), COPY, "{name}: Flit's own answer stays the fallback");
        assert_eq!(app.drop_onto(), Some(p(&format!(r"C:\x\{name}"))));
        app.drag(&DragEvent::Leave, t0());
        assert_eq!(app.drop_onto(), None);
    }
    let at = row(&app, "script.py");
    enter(&mut app, at);
    app.drag(&DragEvent::Drop { x: at.0, y: at.1, mods: NO, allowed: all }, t0());
    assert_eq!(op_list(&mut app), [FileOp::Copy { items: vec![p(r"D:\in\f.txt")], dest: p(r"C:\x") }], "without a handler the drop lands in the folder as before");
    for name in ["sub", "run.exe"] {
        let at = row(&app, name);
        enter(&mut app, at);
        assert_eq!(app.drop_onto(), None, "{name}");
        app.drag(&DragEvent::Leave, t0());
    }
    let at = row(&app, "script.py");
    app.drag(&DragEvent::Enter { paths: vec![p(r"C:\x\script.py")], x: at.0, y: at.1, mods: NO, allowed: all, right: false }, t0());
    assert_eq!(app.drop_onto(), None, "a file is not dropped on itself");
    app.drag(&DragEvent::Leave, t0());
    app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: at.0, y: at.1, mods: NO, allowed: all, right: true }, t0());
    assert_eq!(app.drop_onto(), None, "a right drag keeps Flit's own menu");
    app.drag(&DragEvent::Leave, t0());
    let mut app = app_at("/home/u");
    app.caps.unix_paths = true;
    deliver(&mut app, &[("script.py", false)]);
    let at = row_xy(&app, 0);
    app.drag(&DragEvent::Enter { paths: vec![p("/tmp/f.txt")], x: at.0, y: at.1, mods: NO, allowed: all, right: false }, t0());
    assert_eq!(app.drop_onto(), None);
}

#[test]
fn desktop_listing_labels_sort_and_select_by_title_but_f2_edits_the_actual_file() {
    let mut app = app_at("/tmp/launchers");
    app.caps.unix_paths = true;
    let t = app.tab();
    let mut listing = Listing::default();
    let launcher = listing.push(b"z-browser.desktop", Meta::default());
    listing.set_label(launcher, "Alpha Browser");
    listing.push(b"middle.txt", Meta::default());
    let mut keys = SortKeys::new(CaseMode::Sensitive);
    keys.push(b"z-browser.desktop");
    keys.push(b"middle.txt");
    keys.relabel(&listing);
    let view = ViewSpec::default();
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    t.slot.publish(Arc::new(DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO }));
    app.poll();
    app.drain_effects().for_each(drop);
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!(v.rows(), &[launcher, 1]);
    assert_eq!(app.shown_name(v.listing(), launcher), "Alpha Browser");
    app.opts.char_key = crate::ui::options::CharKey::Select;
    key(&mut app, Key::End, NO);
    typed(&mut app, "a");
    assert_eq!(selected(&app), ["z-browser.desktop"]);
    for needle in ["Alpha", "z-browser"] {
        let v = app.tabs[app.cur].view.as_mut().unwrap();
        v.set_filter(needle);
        assert_eq!(v.rows(), &[launcher]);
    }
    app.tabs[app.cur].view.as_mut().unwrap().set_filter("");
    key(&mut app, Key::F(2), NO);
    assert_eq!(app.rename_text(), Some("z-browser.desktop"));
    typed(&mut app, "new");
    assert_eq!(app.rename_text(), Some("new.desktop"));
    key(&mut app, Key::Enter, NO);
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Rename { item: p("/tmp/launchers/z-browser.desktop"), name: b"new.desktop".to_vec() }]);
}
