use super::*;
use crate::listing::{ChangeKind, DirChange, DirSnapshot, Listing, Meta};
use crate::provider::ProviderEvent;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::ui::Persisted;
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
    assert!(same_volume(&p(r"C:\a"), &p(r"c:\b\c")));
    assert!(!same_volume(&p(r"C:\a"), &p(r"D:\a")));
    assert!(same_volume(&p(r"\\srv\share\a"), &p(r"\\SRV\Share\b")));
    assert!(!same_volume(&p(r"\\srv\one\a"), &p(r"\\srv\two\a")));
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
    assert_eq!(app.tab().error.as_deref(), Some(crate::undo::NOT_RECYCLED));
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

#[test]
fn rename_in_wsl_escapes_windows_characters() {
    let mut app = app_at("wsl://Ubuntu/home");
    deliver(&mut app, &[("a", false)]);
    select(&mut app, &["a"]);
    key(&mut app, Key::F(2), NO);
    typed(&mut app, "x:y");
    key(&mut app, Key::Enter, NO);
    let [(_, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::Rename { item: p(r"\\wsl.localhost\Ubuntu\home\a"), name: "x\u{F03A}y".as_bytes().to_vec() });
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
    assert_eq!(app.drop_highlight(), Some(None));
    assert_eq!(app.drag(&DragEvent::Over { x: empty.0, y: empty.1, mods: SHIFT, allowed: all }, t0()), MOVE);
    assert_eq!(app.drag(&DragEvent::Drop { x: empty.0, y: empty.1, mods: SHIFT, allowed: all }, t0()), MOVE);
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Move { items: vec![p(r"D:\in\f.txt")], dest: p(r"C:\x") }]);
    assert_eq!(app.drop_highlight(), None);

    let sub = row_xy(&app, 0);
    assert_eq!(enter(&mut app, vec![p(r"C:\y\g.txt")], sub, NO), MOVE);
    assert_eq!(app.drop_highlight(), Some(Some(0)));
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
    assert_eq!(app.drop_highlight(), None);
    app.drag(&DragEvent::Drop { x: title.0, y: title.1, mods: NO, allowed: all }, t0());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { path, .. }, .. } if path == p(r"D:\docs"))));
    assert_eq!(app.current_location(), &Location::parse(r"D:\docs").unwrap());
}
