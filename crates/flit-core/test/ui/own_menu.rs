use super::*;
use crate::input::{Key, Mods, MouseButton};
use crate::provider::ProviderRequest;
use crate::text::tests::Mono;
use crate::ui::{Effect, Persisted};
use crate::{FsPath, Location};

fn cmd(label: &str) -> MenuItem {
    MenuItem::command(label, String::new())
}

fn app() -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

fn shown(app: &mut App, launches: Vec<Launch>) {
    let items = vec![cmd("Copy path"), MenuItem::Separator, MenuItem::Submenu { label: "Open with".into(), items: vec![cmd("Editor"), cmd("Viewer")] }];
    app.on_event(&Event::ShowMenu { items, x: 100.0, y: 100.0, launches, kind: crate::ui::MenuKind::None, shell: Vec::new(), pending: false }, Instant::now());
    let mut g = Gfx::new(Mono, 256);
    app.frame(&mut g, Instant::now());
}

fn key(app: &mut App, key: Key) {
    app.on_event(&Event::Key { key, mods: Mods::default(), pressed: true }, Instant::now());
}

#[test]
fn preedit_text_is_composed_inline_at_the_caret_until_it_is_committed() {
    use crate::input::Preedit;
    use crate::ui::field::{compose, EditLine};
    let line = EditLine { text: "ab", caret: 1, selection: 1..1 };
    let p = Preedit::new("にほ".into(), Some("に".len()));
    assert_eq!(compose(&line, &p), ("aにほb".to_owned(), 1 + "に".len(), 1..1 + "にほ".len()));
    assert_eq!(compose(&line, &"にほ".into()).1, 1 + "にほ".len(), "without a cursor the caret ends the preedit");
    assert_eq!(Preedit::new("にほ".into(), Some(1)).cursor, None, "a cursor inside a character is dropped");
    let mut app = app();
    let mut g = Gfx::new(Mono, 256);
    app.open_goto();
    app.on_event(&Event::Preedit(p.clone()), Instant::now());
    assert_eq!(app.preedit, p);
    app.frame(&mut g, Instant::now());
    app.on_event(&Event::Preedit(String::new().into()), Instant::now());
    assert!(app.preedit.is_empty());
    app.frame(&mut g, Instant::now());
}

#[test]
fn commands_are_counted_through_submenus() {
    assert_eq!(commands(&[cmd("a"), MenuItem::Separator, MenuItem::Submenu { label: "s".into(), items: vec![cmd("b"), cmd("c")] }]), 3);
}

#[test]
fn escape_and_clicks_outside_close_the_menu_without_a_command() {
    let mut app = app();
    shown(&mut app, Vec::new());
    assert!(app.own_menu_open());
    key(&mut app, Key::Escape);
    assert!(!app.own_menu_open());
    shown(&mut app, Vec::new());
    app.on_event(&Event::MouseDown { x: 900.0, y: 550.0, button: MouseButton::Left, clicks: 1 }, Instant::now());
    assert!(!app.own_menu_open());
    assert!(app.drain_effects().all(|e| matches!(e, Effect::Icon(_))));
}

#[test]
fn trailing_commands_launch_their_programs_and_the_rest_are_flit_commands() {
    let mut app = app();
    let editor = Launch::With { program: FsPath::parse("/usr/share/applications/editor.desktop").unwrap(), args: vec![FsPath::parse("/x/a.txt").unwrap()] };
    let viewer = Launch::With { program: FsPath::parse("/usr/share/applications/viewer.desktop").unwrap(), args: Vec::new() };
    shown(&mut app, vec![editor.clone(), viewer]);
    for k in [Key::Down, Key::Down, Key::Right, Key::Enter] {
        key(&mut app, k);
    }
    assert!(!app.own_menu_open());
    let launched: Vec<Launch> = app.drain_effects().filter_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } => Some(launch),
        _ => None,
    }).collect();
    assert_eq!(launched, [editor]);
    shown(&mut app, Vec::new());
    key(&mut app, Key::Down);
    key(&mut app, Key::Enter);
    assert!(!app.own_menu_open());
}

fn pending(app: &mut App) {
    let items = vec![cmd("Copy path"), cmd("Rename")];
    app.on_event(&Event::ShowMenu { items, x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::None, shell: Vec::new(), pending: true }, Instant::now());
    app.frame(&mut Gfx::new(Mono, 256), Instant::now());
}

fn late_shell_items(app: &mut App) {
    let items = vec![cmd("Edit with Notepad++"), MenuItem::Submenu { label: "Show more options".into(), items: vec![cmd("Send to")] }];
    app.on_event(&Event::ShellItems { items, shell: vec![7, 9] }, Instant::now());
    app.frame(&mut Gfx::new(Mono, 256), Instant::now());
}

fn picks(app: &mut App) -> Vec<Option<u32>> {
    app.drain_effects().filter_map(|e| if let Effect::ShellPick(p) = e { Some(p) } else { None }).collect()
}

#[test]
fn shell_items_that_arrive_later_join_the_open_menu_with_their_ids() {
    let mut app = app();
    pending(&mut app);
    late_shell_items(&mut app);
    assert_eq!(app.own_menu.as_ref().map(|m| m.items.len()), Some(5), "flit items, a separator, then the shell's");
    for k in [Key::Down, Key::Down, Key::Down, Key::Enter] {
        key(&mut app, k);
    }
    assert!(!app.own_menu_open());
    assert_eq!(picks(&mut app), [Some(7)]);
    pending(&mut app);
    late_shell_items(&mut app);
    for k in [Key::Down, Key::Down, Key::Enter] {
        key(&mut app, k);
    }
    assert_eq!(picks(&mut app), [None], "a Flit command releases the shell menu");
}

#[test]
fn windows_items_join_the_open_group_and_show_more_options_stays_last() {
    let mut app = app();
    let own = vec![cmd("Open"), MenuItem::Submenu { label: "Open in".into(), items: vec![cmd("New tab")] }, MenuItem::Separator, cmd("Copy")];
    app.on_event(&Event::ShowMenu { items: own, x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::File, shell: Vec::new(), pending: true }, Instant::now());
    app.frame(&mut Gfx::new(Mono, 256), Instant::now());
    let shell = vec![cmd("Terminal"), MenuItem::Separator, MenuItem::Submenu { label: "7-Zip".into(), items: vec![cmd("Add")] }, MenuItem::Submenu { label: "Show more options".into(), items: vec![cmd("Send to")] }];
    app.on_event(&Event::ShellItems { items: shell, shell: vec![4, 5, 6] }, Instant::now());
    app.frame(&mut Gfx::new(Mono, 256), Instant::now());
    let labels = app.context_menu_open().unwrap().labels();
    assert_eq!(labels, ["Open", "Open in >", "Terminal", "7-Zip >", "-", "Copy", "-", "Show more options >"]);
    for k in [Key::Down, Key::Down, Key::Down, Key::Enter] {
        key(&mut app, k);
    }
    assert!(!app.own_menu_open());
    assert_eq!(picks(&mut app), [Some(4)], "a lifted Windows item still runs its own verb");
}

#[test]
fn a_menu_still_waiting_for_shell_items_releases_them_and_ignores_late_ones() {
    let mut app = app();
    pending(&mut app);
    key(&mut app, Key::Escape);
    assert_eq!(picks(&mut app), [None]);
    late_shell_items(&mut app);
    assert!(!app.own_menu_open());
    shown(&mut app, Vec::new());
    late_shell_items(&mut app);
    assert_eq!(app.own_menu.as_ref().map(|m| m.items.len()), Some(3), "a menu without shell items takes none");
    pending(&mut app);
    pending(&mut app);
    assert_eq!(picks(&mut app), [None], "a new menu releases the one still waiting");
}

#[test]
fn a_right_click_on_another_file_closes_the_open_menu_and_asks_for_that_file() {
    let mut a = crate::ui::harness::app(&["a.txt", "b.txt", "c.txt"]);
    let mut g = Gfx::new(Mono, 256);
    let right = |a: &mut App, x: f32, y: f32| {
        a.on_event(&Event::MouseMove { x, y }, Instant::now());
        a.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, Instant::now());
        a.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, Instant::now());
        a.drain_effects().find_map(|e| match e {
            Effect::ShellMenu(m) => Some(m.items),
            _ => None,
        })
    };
    let (r0, r2) = (a.row_rect(0).unwrap(), a.row_rect(2).unwrap());
    let first = right(&mut a, r0.x + 700.0, r0.y + r0.h / 2.0).unwrap();
    assert_eq!(first, [FsPath::parse(r"C:\Work\a.txt").unwrap()]);
    a.on_event(&Event::ShowMenu { items: vec![cmd("Open")], x: r0.x + 700.0, y: r0.y, launches: Vec::new(), kind: crate::ui::MenuKind::File, shell: vec![1], pending: false }, Instant::now());
    a.frame(&mut g, Instant::now());
    let (x, y) = (r2.x + 60.0, r2.y + r2.h / 2.0);
    assert!(a.own_menu_open() && !a.over_own_menu(x, y));
    let second = right(&mut a, x, y);
    assert!(!a.own_menu_open(), "the old menu closes");
    assert_eq!(second, Some(vec![FsPath::parse(r"C:\Work\c.txt").unwrap()]), "the new menu is for the file under the pointer");
    let v = a.tab().view.as_ref().unwrap();
    assert!(v.is_selected(v.rows()[2]) && v.selected_count() == 1);
}

#[test]
fn a_menu_opens_at_the_size_its_kind_had_and_text_typed_early_survives_the_shell_items() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 256);
    let target = crate::ui::ShellMenu { dir: FsPath::parse(r"C:\x").unwrap(), items: vec![FsPath::parse(r"C:\x\a.TXT").unwrap()], x: 100.0, y: 100.0, extended: false, menu: Vec::new(), inline: false, shell: true, kind: crate::ui::MenuKind::File };
    assert_eq!(shape_key(&target), "file.txt");
    let mut open = |app: &mut App| {
        app.remember_shape_key(&target);
        app.on_event(&Event::ShowMenu { items: vec![cmd("Open")], x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::File, shell: Vec::new(), pending: true }, Instant::now());
        app.frame(&mut g, Instant::now());
        app.context_menu_open().map(|c| c.rect).unwrap()
    };
    let arrive = |app: &mut App| {
        let items = vec![cmd("Scan"), MenuItem::Separator, MenuItem::Submenu { label: "Show more options".into(), items: vec![cmd("Send to")] }];
        app.on_event(&Event::ShellItems { items, shell: vec![7, 9] }, Instant::now());
        app.frame(&mut Gfx::new(Mono, 256), Instant::now());
        app.context_menu_open().map(|c| c.rect).unwrap()
    };
    let unknown = open(&mut app);
    let full = arrive(&mut app);
    assert!(full.h > unknown.h, "the first menu of a kind grows when the shell joins");
    key(&mut app, Key::Escape);
    let early = open(&mut app);
    assert_eq!((early.h, early.w), (full.h, full.w), "the next one opens at the size it will have");
    assert_eq!(app.context_menu_open().unwrap().labels(), ["Open", "", "-", ""], "neutral rows hold the shell's places");
    app.on_event(&Event::Char('s'), Instant::now());
    let late = arrive(&mut app);
    let c = app.context_menu_open().unwrap();
    assert_eq!(c.query.text(), "s", "text typed before the shell items is kept");
    assert!(c.labels().iter().any(|l| l.contains("Scan")), "{:?}", c.labels());
    assert!(late.h <= full.h);
    assert_eq!(commands(&app.own_menu.as_ref().unwrap().items), 3, "no placeholder is a command");
}

#[test]
fn a_click_on_a_submenu_of_the_context_menu_redraws_it_open_at_once() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 256);
    let items = vec![cmd("Paste"), MenuItem::Submenu { label: "New".into(), items: vec![cmd("Folder"), cmd("Text Document")] }];
    app.on_event(&Event::ShowMenu { items, x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::Background, shell: Vec::new(), pending: false }, Instant::now());
    app.frame(&mut g, Instant::now());
    let c = app.context_menu_open().unwrap();
    let x = c.rect.x + c.rect.w / 2.0;
    let new_row = c.labels().iter().position(|l| l == "New >").unwrap();
    let y = (0..c.rect.h as i32).map(|dy| c.rect.y + dy as f32).find(|&y| c.hit(x, y) == Some(crate::ui::context::CtxHit::Row(new_row))).unwrap();
    for ev in [Event::MouseMove { x, y }, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }] {
        app.on_event(&ev, Instant::now());
        app.frame(&mut g, Instant::now());
    }
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
    assert!(app.context_menu_open().unwrap().is_open("New"));
    assert!(app.dirty, "the opened submenu is drawn on the next frame, not after the pointer moves again");
}
