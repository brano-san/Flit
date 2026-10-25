use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::provider::ProviderRequest;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::ui::{Effect, Persisted, ShellMenu};
use crate::FsPath;
use std::sync::Arc;
use std::time::Duration;

const FOLDER: usize = 0;
const FILE: usize = 1;

fn app_with(items: &[(&str, bool)]) -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    app
}

fn right_click(app: &mut App, row: Option<usize>, shift: bool) -> ShellMenu {
    let l = &app.layout;
    let (x, y) = (l.cols[0].x + 200.0, row.map_or(l.list.y + 200.0, |r| l.list.y + (r as f32 + 0.5) * l.row_h));
    app.on_event(&Event::Key { key: Key::Other, mods: Mods { shift, ..Mods::default() }, pressed: false }, Instant::now());
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, Instant::now());
    app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).unwrap()
}

fn commands(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, keys, checked } => vec![format!("{label}{}{}", if keys.is_empty() { String::new() } else { format!("\t{keys}") }, if *checked { " *" } else { "" })],
            MenuItem::Submenu { items, .. } => commands(items),
            MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn label_of(c: &str) -> &str {
    c.split('\t').next().unwrap().trim_end_matches(" *")
}

fn run(app: &mut App, menu: &ShellMenu, label: &str) {
    let all = commands(&menu.menu);
    let k = all.iter().position(|c| label_of(c) == label).unwrap_or_else(|| panic!("{label} not in {all:?}"));
    app.on_event(&Event::MenuCommand(k), Instant::now());
}

fn tidy(items: &[MenuItem]) -> bool {
    let seps: Vec<bool> = items.iter().map(|i| *i == MenuItem::Separator).collect();
    !seps.first().copied().unwrap_or(false) && !seps.last().copied().unwrap_or(false) && !seps.windows(2).any(|w| w[0] && w[1])
}

#[test]
fn item_menu_lists_flit_commands_with_their_shortcuts() {
    let mut app = app_with(&[("a.txt", false), ("sub", true)]);
    let file = right_click(&mut app, Some(FILE), false);
    assert_eq!(file.items, [FsPath::parse(r"C:\x\a.txt").unwrap()]);
    assert!(!file.inline);
    let labels = commands(&file.menu);
    assert_eq!(labels[0], "Open\tEnter");
    for want in ["Cut\tCtrl+X", "Copy\tCtrl+C", "Copy path\tCtrl+Shift+C", "Create shortcut", "Rename\tF2", "Delete\tDel", "New folder with selection", "New alternate data stream", "Properties\tAlt+Enter"] {
        assert!(labels.iter().any(|l| l == want), "{want} missing from {labels:?}");
    }
    assert!(!labels.iter().any(|l| l.starts_with("Open in new") || l.contains("sidebar") || l.starts_with("Paste")), "{labels:?}");
    assert!(tidy(&file.menu));
    let folder = right_click(&mut app, Some(FOLDER), false);
    let labels = commands(&folder.menu);
    for want in ["Open in new tab", "Open in new window", "Paste\tCtrl+Shift+V", "Pin to sidebar", "Open in terminal\tCtrl+`"] {
        assert!(labels.iter().any(|l| l == want), "{want} missing from {labels:?}");
    }
    assert!(!labels.iter().any(|l| l.starts_with("New alternate")), "{labels:?}");
}

#[test]
fn menu_commands_run_their_actions() {
    let mut app = app_with(&[("a.txt", false), ("sub", true)]);
    let menu = right_click(&mut app, Some(FOLDER), false);
    run(&mut app, &menu, "Open in new tab");
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(app.tabs()[1].location(), &Location::parse(r"C:\x\sub").unwrap());
    app.select_tab(0);
    app.drain_effects().for_each(drop);
    let menu = right_click(&mut app, Some(FOLDER), false);
    run(&mut app, &menu, "Open in new window");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::NewWindow(r) if r.len() == 1)));
    let menu = right_click(&mut app, Some(FOLDER), false);
    run(&mut app, &menu, "Pin to sidebar");
    let sub = Location::parse(r"C:\x\sub").unwrap();
    assert!(app.drain_effects().any(|e| matches!(&e, Effect::Pinned(l) if *l == [sub.clone()])));
    let menu = right_click(&mut app, Some(FOLDER), false);
    run(&mut app, &menu, "Unpin from sidebar");
    assert!(app.drain_effects().any(|e| matches!(&e, Effect::Pinned(l) if l.is_empty())));
    let menu = right_click(&mut app, Some(FILE), false);
    run(&mut app, &menu, "Create shortcut");
    let link = app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Op { op: crate::provider::FileOp::Link { items, dest }, .. }, .. } => Some((items, dest)),
        _ => None,
    });
    assert_eq!(link, Some((vec![FsPath::parse(r"C:\x\a.txt").unwrap()], FsPath::parse(r"C:\x").unwrap())));
    app.on_event(&Event::MenuCommand(999), Instant::now());
}

#[test]
fn background_menu_offers_sorting_new_items_paste_and_terminal() {
    let mut app = app_with(&[("a.txt", false)]);
    let menu = right_click(&mut app, None, false);
    assert!(menu.items.is_empty());
    assert!(matches!(&menu.menu[0], MenuItem::Submenu { label, .. } if label == "Layout"));
    assert!(matches!(&menu.menu[1], MenuItem::Submenu { label, .. } if label == "Sort by"));
    let labels = commands(&menu.menu);
    for want in ["Details\tCtrl+Shift+1 *", "Grid\tCtrl+Shift+5", "Larger items\tCtrl++", "Name *", "Date modified", "Descending", "Refresh\tF5", "Folder\tCtrl+Shift+N", "Shortcut", "Paste\tCtrl+V", "Open in terminal\tCtrl+`", "Pin to sidebar\tCtrl+D", "Copy path\tCtrl+Shift+C", "Properties\tAlt+Enter"] {
        assert!(labels.iter().any(|l| l == want), "{want} missing from {labels:?}");
    }
    run(&mut app, &menu, "Grid");
    assert_eq!((app.tab().mode, app.layout.mode), (LayoutMode::Grid, LayoutMode::Grid));
    let menu = right_click(&mut app, None, false);
    assert!(commands(&menu.menu).iter().any(|l| l == "Grid\tCtrl+Shift+5 *"));
    run(&mut app, &menu, "Details");
    run(&mut app, &menu, "Date modified");
    assert_eq!(app.tab().view_spec.sort.field, SortField::Modified);
    let menu = right_click(&mut app, None, false);
    assert!(commands(&menu.menu).iter().any(|l| l == "Date modified *"));
    run(&mut app, &menu, "Descending");
    assert!(app.tab().view_spec.sort.descending);
    let menu = right_click(&mut app, None, false);
    run(&mut app, &menu, "Folder");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { op: crate::provider::FileOp::NewFolder { .. }, .. }, .. })));
}

#[test]
fn hidden_items_and_inline_shell_items_follow_the_core_flags() {
    let mut app = app_with(&[("a.txt", false), ("sub", true)]);
    app.menu_hidden = vec!["item.open", "file.cut", "file.copy", "item.copy_path", "file.shortcut", "file.rename", "item.trash", "file.new_item", "nav.refresh"];
    let file = right_click(&mut app, Some(FILE), false);
    let labels = commands(&file.menu);
    assert!(!labels.iter().any(|l| ["Open", "Cut", "Copy", "Rename"].contains(&label_of(l))), "{labels:?}");
    assert!(tidy(&file.menu), "{:?}", file.menu);
    let background = right_click(&mut app, None, false);
    assert!(!commands(&background.menu).iter().any(|l| l.starts_with("Folder") || l.starts_with("Refresh")));
    assert!(tidy(&background.menu), "{:?}", background.menu);
    assert!(right_click(&mut app, None, true).inline, "Shift shows every shell item in place");
    app.on_event(&Event::Key { key: Key::F(10), mods: Mods { shift: true, ..Mods::default() }, pressed: true }, Instant::now());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::ShellMenu(m) if !m.inline && !m.extended)), "Shift+F10 is the keyboard menu, not an extended one");
    app.shell_items_inline = true;
    assert!(right_click(&mut app, Some(FILE), false).inline);
}
