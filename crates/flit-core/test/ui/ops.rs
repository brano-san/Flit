use super::*;
use crate::listing::{ChangeKind, DirChange, DirSnapshot, Listing, Meta};
use crate::provider::ProviderEvent;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::ui::{Drive, Persisted};
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
    (l.cols[0].x + 200.0, l.list.y + (row as f32 + 0.5) * l.row_h)
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
    let drive = |path: &str| Drive { path: p(path), label: String::new(), total: 0, free: 0, removable: false, unmounted: false, kind: None, remote: None };
    let mounts = [drive("/"), drive("/home"), drive("/media/u/USB"), drive(r"C:\")];
    assert!(same_volume(&p("/home/a"), &p("/home/b/c"), &mounts));
    assert!(!same_volume(&p("/home/a"), &p("/media/u/USB/x"), &mounts));
    assert!(!same_volume(&p("/etc"), &p("/home"), &mounts));
    assert!(same_volume(&p("/etc"), &p("/usr/lib"), &mounts));
    assert!(!same_volume(&p("/media/u/usb/x"), &p("/media/u/USB/x"), &mounts), "mount points are case-sensitive");
    assert!(same_volume(&p("/a"), &p("/b"), &[]), "without a mount list everything is one volume");
    assert!(!same_volume(&p("/a"), &p(r"C:\a"), &mounts));
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
    typed(&mut app, "bad:name");
    key(&mut app, Key::Enter, NO);
    assert!(app.is_renaming(), "an invalid name keeps the editor open");
    assert!(app.tab().error.as_deref().is_some_and(|e| e.contains("cannot contain")));
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
    assert_eq!(route(ubuntu(), FileOp::Copy { items: vec![p("/a")], dest: p(r"C:\b") }, false), None);
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
    key(&mut app, Key::Enter, Mods { shift: true, ..ALT });
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Properties(p) if p == [FsPath::parse(r"C:\x\a.txt").unwrap()])));
    key(&mut app, Key::Enter, ALT);
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
fn drops_copy_or_move_by_volume_and_modifiers() {
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("sub", true), ("a.txt", false)]);
    let empty = (300.0, app.layout.list.y + 200.0);
    let all = COPY | MOVE | LINK;
    let enter = |app: &mut App, paths: Vec<FsPath>, at: (f32, f32), mods: Mods| app.drag(&DragEvent::Enter { paths, x: at.0, y: at.1, mods, allowed: all }, t0());
    assert_eq!(enter(&mut app, vec![p(r"D:\in\f.txt")], empty, NO), COPY);
    assert_eq!(app.drop_spot(), Some(Spot::List));
    assert_eq!(app.drag(&DragEvent::Over { x: empty.0, y: empty.1, mods: SHIFT, allowed: all }, t0()), MOVE);
    assert_eq!(app.drag(&DragEvent::Drop { x: empty.0, y: empty.1, mods: SHIFT, allowed: all }, t0()), MOVE);
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Move { items: vec![p(r"D:\in\f.txt")], dest: p(r"C:\x") }]);
    assert_eq!(app.drop_spot(), None);

    let sub = row_xy(&app, 0);
    assert_eq!(enter(&mut app, vec![p(r"C:\y\g.txt")], sub, NO), MOVE);
    assert_eq!(app.drop_spot(), Some(Spot::Row(0)));
    assert_eq!(app.drag(&DragEvent::Drop { x: sub.0, y: sub.1, mods: CTRL, allowed: all }, t0()), COPY);
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Copy { items: vec![p(r"C:\y\g.txt")], dest: p(r"C:\x\sub") }]);

    assert_eq!(enter(&mut app, vec![p(r"C:\x\a.txt")], empty, NO), NONE, "moving into its own folder does nothing");
    assert_eq!(app.drag(&DragEvent::Over { x: empty.0, y: empty.1, mods: ALT, allowed: all }, t0()), LINK);
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
    assert_eq!(app.drag(&DragEvent::Enter { paths: vec![p(r"D:\docs\r.txt")], x: title.0, y: title.1, mods: NO, allowed: all }, t0()), drop_effect::COPY);
    assert_eq!(app.drop_spot(), None);
    app.drag(&DragEvent::Drop { x: title.0, y: title.1, mods: NO, allowed: all }, t0());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { path, .. }, .. } if path == p(r"D:\docs"))));
    assert_eq!(app.current_location(), &Location::parse(r"D:\docs").unwrap());
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
    let effect = app.drag(&DragEvent::Enter { paths, x: at.0, y: at.1, mods: NO, allowed: all }, t0());
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
    app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: sub.0, y: sub.1, mods: NO, allowed: all }, start);
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
    for (k, m) in [(Key::Char('C'), CTRL), (Key::Char('X'), CTRL), (Key::Enter, ALT), (Key::F(10), SHIFT)] {
        key(&mut app, k, m);
        assert!(app.drain_effects().all(|e| !matches!(e, Effect::SetClipboardFiles { .. } | Effect::Properties(_) | Effect::ShellMenu(_))));
        assert_eq!(app.tab().error.as_deref(), Some(BIN_ONLY.t()));
    }
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

#[test]
fn the_status_bar_buttons_and_headers_of_the_bin_work_with_the_mouse() {
    let mut app = bin_app();
    select(&mut app, &["old"]);
    let buttons = app.layout.bin;
    assert!(buttons.iter().all(|r| r.w > 0.0) && app.layout.sizes.w == 0.0);
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
    let enter = |app: &mut App, paths: Vec<FsPath>, at: (f32, f32)| app.drag(&DragEvent::Enter { paths, x: at.0, y: at.1, mods: CTRL, allowed: all }, t0());
    assert_eq!(enter(&mut app, vec![p(r"D:\in\f.txt")], at), MOVE);
    assert_eq!(app.drop_spot(), Some(Spot::Side(k)));
    assert_eq!(app.drag(&DragEvent::Drop { x: at.0, y: at.1, mods: NO, allowed: all }, t0()), MOVE);
    assert_eq!(op_list(&mut app), [FileOp::Trash(vec![p(r"D:\in\f.txt")])]);
    assert_eq!(enter(&mut app, vec![p(BIN_REPORT.0)], at), NONE, "what is already in the bin stays there");
    app.drag(&DragEvent::Leave, t0());
    assert_eq!(app.drag(&DragEvent::Enter { paths: vec![p(r"D:\in\f.txt")], x: at.0, y: at.1, mods: NO, allowed: COPY }, t0()), NONE);
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
    let t = |label: &str, ext: &str, template| NewTemplate { label: label.into(), ext: ext.into(), stem: format!("New {label}"), template };
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
fn changing_the_extension_needs_a_second_enter() {
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
    assert!(app.tab().error.as_deref().is_some_and(|e| e.contains("extension")));
    key(&mut app, Key::Enter, NO);
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\notes.txt"), name: b"notes.md".to_vec() }]);
    rename(&mut app, "notes.txt", "draft.TXT");
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\notes.txt"), name: b"draft.TXT".to_vec() }]);
    rename(&mut app, "notes.txt", "notes");
    assert!(app.is_renaming(), "dropping the extension warns too");
    key(&mut app, Key::Escape, NO);
    assert!(!app.is_renaming() && ops(&mut app).is_empty());
    app.warn_extension_change = false;
    rename(&mut app, "notes.txt", "notes.log");
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\notes.txt"), name: b"notes.log".to_vec() }]);
    app.warn_extension_change = true;
    rename(&mut app, "src", "src.old");
    assert_eq!(op_list(&mut app), [FileOp::Rename { item: p(r"C:\x\src"), name: b"src.old".to_vec() }], "folders have no extension");
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
fn the_folder_icon_command_asks_for_an_icon_location_and_writes_it_for_the_selected_folder() {
    assert_eq!(icon_location(r#" "C:\Windows\System32\imageres.dll" , -113 "#), Some((p(r"C:\Windows\System32\imageres.dll"), -113)));
    assert_eq!(icon_location(r"C:\i\a,b.ico"), Some((p(r"C:\i\a,b.ico"), 0)));
    assert_eq!(icon_location(r"icons\a.ico"), None);
    use crate::listing::{attr, Entry};
    use crate::ui::{icon_key, IconKind};
    let dir = |attrs| Entry { size: 0, modified: 0, created: 0, name_off: 0, attrs, name_len: 0, ext_len: 0, tags: 0 };
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
    key(&mut app, Key::Char('C'), CTRL_ALT);
    assert_eq!(copied(&mut app).as_deref(), Some(r#""C:\x\sub""#));
    select(&mut app, &["sub", "a.txt"]);
    key(&mut app, Key::Char('C'), CTRL_ALT);
    assert_eq!(copied(&mut app).as_deref(), Some("\"C:\\x\\sub\"\r\n\"C:\\x\\a.txt\""));
    select(&mut app, &[]);
    key(&mut app, Key::Char('C'), CTRL_ALT);
    assert_eq!(copied(&mut app).as_deref(), Some(r#""C:\x""#), "with nothing selected the folder itself is copied");
    key(&mut app, Key::Char('C'), CTRL_SHIFT);
    assert_eq!(copied(&mut app).as_deref(), Some(r"C:\x"));
}

#[test]
fn cut_items_stay_dimmed_until_the_clipboard_changes() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Char('X'), CTRL);
    assert!(app.is_cut(b"a.txt") && !app.is_cut(b"b.txt"));
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
    key(&mut app, Key::Char('`'), CTRL);
    assert_eq!(launches(&mut app), [Launch::Terminal { dirs: vec![p(r"C:\x")], admin: false }]);
    select(&mut app, &["a.txt"]);
    key(&mut app, Key::Char('`'), CTRL_SHIFT);
    assert_eq!(launches(&mut app), [Launch::Terminal { dirs: vec![p(r"C:\x")], admin: true }]);
    let mut all = dirs.to_vec();
    all.push("a.txt");
    select(&mut app, &all);
    key(&mut app, Key::Char('`'), CTRL);
    let first: Vec<FsPath> = dirs[..5].iter().map(|d| p(&format!(r"C:\x\{d}"))).collect();
    assert_eq!(launches(&mut app), [Launch::Terminal { dirs: first, admin: false }]);
    assert_eq!(app.tab().error.as_deref(), Some("Opened the first 5 of 7 folders in the terminal"));
    let mut app = app_at("This PC");
    deliver(&mut app, &[("C:", true)]);
    key(&mut app, Key::Char('`'), CTRL);
    assert!(launches(&mut app).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some("This location cannot be opened in a terminal"));
}

#[test]
fn files_dropped_on_a_program_are_passed_to_it() {
    use drop_effect::*;
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false), ("run.CMD", false), ("tool.exe", false)]);
    let all = COPY | MOVE | LINK;
    let enter = |app: &mut App, paths: Vec<FsPath>, at: (f32, f32)| app.drag(&DragEvent::Enter { paths, x: at.0, y: at.1, mods: NO, allowed: all }, t0());
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
