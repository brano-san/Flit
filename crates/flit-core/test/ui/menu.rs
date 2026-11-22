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

#[test]
fn windows_paste_menu_exposes_all_link_kinds_but_disables_unknown_sources() {
    let mut app = app_with(&[]);
    let menu = app.build_menu(false);
    let paste = menu
        .iter()
        .find_map(|item| match item {
            MenuItem::Submenu { label, items } if label == "Paste as" => Some(items),
            _ => None,
        })
        .expect("Paste as submenu");
    assert_eq!(commands(paste), ["Shortcut", "Symbolic link", "Junction", "Hard link"]);
    assert!(paste.iter().skip(1).all(|item| matches!(item, MenuItem::Command { disabled: true, .. })));
}

fn paste_link_probe(app: &mut App) -> (ShellMenu, u64) {
    app.context_menu();
    let effects: Vec<_> = app.drain_effects().collect();
    let generation = effects.iter().find_map(|e| if let Effect::LinkChoices { generation, .. } = e { Some(*generation) } else { None }).unwrap();
    let menu = effects.into_iter().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).unwrap();
    (menu, generation)
}

#[test]
fn a_slow_link_probe_keeps_paste_usable_and_never_runs_a_disabled_link() {
    let mut app = app_with(&[]);
    let (menu, _) = paste_link_probe(&mut app);
    run(&mut app, &menu, "Symbolic link");
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { .. }, .. })));
    run(&mut app, &menu, "Paste");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { op: crate::provider::FileOp::Paste { .. }, .. }, .. })));
}

#[test]
fn link_probe_answers_enable_only_current_clipboard_sources_and_queue_the_selected_kind() {
    use crate::provider::{FileOp, LinkChoices, LinkKind};
    let mut app = app_with(&[]);
    let (_, old) = paste_link_probe(&mut app);
    app.on_event(&Event::ClipboardChanged(20), Instant::now());
    let (menu, current) = paste_link_probe(&mut app);
    app.on_event(&Event::LinkChoices { generation: old, choices: LinkChoices([true; 4]) }, Instant::now());
    run(&mut app, &menu, "Hard link");
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { .. }, .. })));
    app.on_event(&Event::LinkChoices { generation: current, choices: LinkChoices([true, true, false, true]) }, Instant::now());
    run(&mut app, &menu, "Junction");
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { .. }, .. })));
    run(&mut app, &menu, "Hard link");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { op: FileOp::PasteLinkAs { kind: LinkKind::Hard, .. }, .. }, .. })));
    assert_eq!(app.running_jobs(), 1);
}

#[test]
fn unix_and_virtual_locations_do_not_offer_windows_link_kinds() {
    let mut app = app_with(&[]);
    app.caps = crate::caps::Caps { unix_paths: true };
    assert!(!commands(&app.build_menu(false)).iter().any(|s| s.contains("Junction") || s.contains("Hard link")));
    for path in [crate::Location::computer(), crate::Location::recycle_bin()] {
        let mut app = App::new(Persisted { path, ..Default::default() }, Vec::new(), 0, false, 1.0, (400, 300), Instant::now());
        let menu = app.build_menu(false);
        assert!(!menu.iter().any(|m| matches!(m, MenuItem::Submenu { label, .. } if label == "Paste as")));
    }
}

fn app_with(items: &[(&str, bool)]) -> App {
    let attrs: Vec<(&str, u32)> = items.iter().map(|&(n, dir)| (n, if dir { attr::DIR } else { 0 })).collect();
    app_with_attrs(&attrs)
}

fn app_with_attrs(items: &[(&str, u32)]) -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, attrs) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs, ..Default::default() });
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
            MenuItem::Command { label, keys, checked, .. } => vec![format!("{label}{}{}", if keys.is_empty() { String::new() } else { format!("\t{keys}") }, if *checked { " *" } else { "" })],
            MenuItem::Submenu { items, .. } => commands(items),
            MenuItem::Header(_) | MenuItem::Separator => Vec::new(),
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
fn built_in_context_commands_have_icons_in_every_context() {
    fn check(items: &[MenuItem]) {
        for item in items {
            match item {
                MenuItem::Command { label, icon, image, .. } => assert!(!icon.is_empty() || image.is_some(), "missing icon: {label}"),
                MenuItem::Submenu { items, .. } => check(items),
                _ => {}
            }
        }
    }
    let mut app = app_with(&[("a.txt", false), ("sub", true)]);
    for unix_paths in [false, true] {
        app.caps = crate::caps::Caps { unix_paths };
        for row in [Some(FILE), Some(FOLDER), None] {
            check(&right_click(&mut app, row, false).menu);
        }
    }
}

#[test]
fn releasing_a_right_click_over_another_item_does_not_open_the_wrong_context() {
    let mut app = app_with(&[("a.txt", false), ("sub", true)]);
    let (x, y) = (app.layout.cols[0].x + 100.0, app.layout.list.y + 0.5 * app.layout.row_h);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y: y + app.layout.row_h, button: MouseButton::Right }, Instant::now());
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::ShellMenu(_))));
}

#[test]
fn item_menu_lists_flit_commands_with_their_shortcuts() {
    let mut app = app_with(&[("a.txt", false), ("sub", true)]);
    let file = right_click(&mut app, Some(FILE), false);
    assert_eq!(file.items, [FsPath::parse(r"C:\x\a.txt").unwrap()]);
    assert!(!file.inline);
    let labels = commands(&file.menu);
    assert_eq!(labels[0], "Open\tEnter");
    for want in ["Cut\tCtrl+X", "Copy\tCtrl+C  Ctrl+Insert", "Copy as path\tCtrl+Shift+C", "Create shortcut", "Rename\tF2", "Delete\tDelete", "New folder with selection", "New alternate data stream", "Properties\tShift+Alt+Enter"] {
        assert!(labels.iter().any(|l| l == want), "{want} missing from {labels:?}");
    }
    assert!(!labels.iter().any(|l| l.starts_with("Open in new") || l.contains("sidebar") || l.starts_with("Paste")), "{labels:?}");
    assert!(tidy(&file.menu));
    let folder = right_click(&mut app, Some(FOLDER), false);
    let labels = commands(&folder.menu);
    for want in ["Open in new tab\tCtrl+Enter", "Open in new window", "Paste\tCtrl+Shift+V", "Pin to sidebar", "Open in terminal", "Change icon"] {
        assert!(labels.iter().any(|l| l == want), "{want} missing from {labels:?}");
    }
    assert!(!labels.iter().any(|l| l.starts_with("New alternate")), "{labels:?}");
    assert!(!commands(&file.menu).iter().any(|l| l == "Change icon"));
    assert!(!labels.iter().any(|l| l == "Restore default icon"), "a plain folder has no custom icon to remove");
}

#[test]
fn a_folder_that_can_carry_a_custom_icon_offers_to_restore_the_default_one() {
    let mut app = app_with_attrs(&[("photos", attr::DIR | attr::READONLY)]);
    let menu = right_click(&mut app, Some(0), false);
    run(&mut app, &menu, "Restore default icon");
    let op = app.drain_effects().find_map(|e| if let Effect::Provider { req: ProviderRequest::Op { op, .. }, .. } = e { Some(op) } else { None });
    assert_eq!(op, Some(crate::provider::FileOp::FolderIcon { folder: FsPath::parse(r"C:\x\photos").unwrap(), icon: None, index: 0 }));
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
    assert!(matches!(&menu.menu[0], MenuItem::Submenu { label, .. } if label == "Sort"));
    assert!(matches!(&menu.menu[1], MenuItem::Submenu { label, .. } if label == "Group"));
    let labels = commands(&menu.menu);
    for want in ["Total Items", "Name *", "Date Modified", "Refresh\tCtrl+R  F5", "Folder\tCtrl+Shift+N", "Shortcut", "Paste\tCtrl+V  Shift+Insert", "Open in terminal", "Pin to sidebar", "Copy as path\tCtrl+Shift+C", "Properties\tShift+Alt+Enter"] {
        assert!(labels.iter().any(|l| l == want), "{want} missing from {labels:?}");
    }
    assert!(!labels.iter().any(|l| l.starts_with("Details") || l.starts_with("Cards")), "the bottom bar switches layouts, so the menu has no View submenu");
    run(&mut app, &menu, "Date Modified");
    assert_eq!(app.tab().view_spec.sort.field, SortField::Modified);
    let menu = right_click(&mut app, None, false);
    assert!(commands(&menu.menu).iter().any(|l| l == "Date Modified *"));
    run(&mut app, &menu, "Date Modified");
    assert!(app.tab().view_spec.sort.descending);
    let menu = right_click(&mut app, None, false);
    run(&mut app, &menu, "Folder");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { op: crate::provider::FileOp::NewFolder { .. }, .. }, .. })));
}

#[test]
fn background_properties_show_the_current_folder_even_with_a_selection() {
    let mut app = app_with(&[("a.txt", false), ("sub", true)]);
    let menu = right_click(&mut app, None, false);
    app.tab_mut().view.as_mut().unwrap().select_all();
    run(&mut app, &menu, "Properties");
    let shown: Vec<Vec<FsPath>> = app.drain_effects().filter_map(|e| if let Effect::Properties(p) = e { Some(p) } else { None }).collect();
    assert_eq!(shown, [vec![FsPath::parse(r"C:\x").unwrap()]]);
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
    assert!(!commands(&background.menu).iter().any(|l| l.starts_with("Folder\t") || l.starts_with("Refresh")));
    assert!(tidy(&background.menu), "{:?}", background.menu);
    assert!(right_click(&mut app, None, true).inline, "Shift shows every shell item in place");
    app.on_event(&Event::Key { key: Key::F(10), mods: Mods { shift: true, ..Mods::default() }, pressed: true }, Instant::now());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::ShellMenu(m) if !m.inline && !m.extended)), "Shift+F10 is the keyboard menu, not an extended one");
    app.shell_items_inline = true;
    assert!(right_click(&mut app, Some(FILE), false).inline);
}

#[test]
fn windows_only_actions_disappear_everywhere_on_linux() {
    let mut app = app_with(&[("a.txt", false), ("sub", true)]);
    let windows = commands(&right_click(&mut app, Some(FILE), false).menu);
    assert!(windows.iter().any(|l| l == "Create shortcut") && windows.iter().any(|l| l == "New alternate data stream"));
    app.caps = crate::caps::Caps { unix_paths: true };
    let file = commands(&right_click(&mut app, Some(FILE), false).menu);
    let folder = commands(&right_click(&mut app, Some(FOLDER), false).menu);
    for gone in ["New alternate data stream", "Change icon"] {
        assert!(!file.iter().chain(&folder).any(|l| l == gone), "{gone} in {file:?} {folder:?}");
    }
    assert!(file.iter().any(|l| l == crate::i18n::S::MenuCreateShortcut.t()), "links are made on Linux too: {file:?}");
    assert!(file.iter().any(|l| l.starts_with("Rename")));
    for id in crate::caps::WINDOWS_ONLY_ACTIONS {
        assert!(!app.available(id), "{id} offered by the palette");
        assert!(!app.run_action(id, Instant::now()), "{id} runs");
    }
    assert!(app.available("file.rename"));
    app.open_palette();
    let ids: Vec<&str> = app.nav.palette.as_ref().unwrap().ids.iter().map(|&i| crate::ui::actions::ACTIONS[i].id).collect();
    assert!(ids.contains(&"file.rename"), "{ids:?}");
    assert!(!ids.iter().any(|id| crate::caps::WINDOWS_ONLY_ACTIONS.contains(id)), "{ids:?}");
}

#[test]
fn folder_sizes_is_a_check_item_in_the_layout_menu_and_keeps_its_shortcut() {
    let mut app = app_with(&[("a.txt", false), ("sub", true)]);
    let checked = |app: &mut App| {
        let items = app.build_layout_menu();
        items.iter().find_map(|i| match i {
            MenuItem::Command { label, checked, .. } if label == "Show folder sizes" => Some(*checked),
            _ => None,
        })
    };
    assert_eq!(checked(&mut app), Some(false));
    let k = app.menu.iter().position(|c| *c == Cmd::Action("view.auto_sizes")).expect("the item runs view.auto_sizes");
    app.menu_command(k, Instant::now());
    assert!(app.prefs.auto_sizes);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::DirSizes { .. }, .. })), "turning it on computes the sizes");
    assert_eq!(checked(&mut app), Some(true));
    assert_eq!(app.settings.keymap.first_text("folder.refresh_stats").as_deref(), Some("Alt+R"));
    app.on_event(&Event::Key { key: Key::Char('R'), mods: Mods { ctrl: false, shift: false, alt: true }, pressed: true }, Instant::now());
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::DirSizes { .. }, .. })), "a pending calculation is not duplicated");
}

fn launched(app: &mut App) -> Vec<crate::provider::Launch> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect()
}

#[test]
fn linux_menus_offer_printing_disk_images_fonts_and_the_system_file_manager() {
    use crate::provider::Launch;
    let mut app = app_with(&[("sub", true), ("a.pdf", false), ("b.txt", false), ("disk.iso", false), ("Sans.ttf", false)]);
    app.caps = crate::caps::Caps { unix_paths: true };
    let pdf = right_click(&mut app, Some(1), false);
    assert!(commands(&pdf.menu).iter().any(|l| l == "Print") && !commands(&pdf.menu).iter().any(|l| l == "Mount disk image"));
    run(&mut app, &pdf, "Print");
    assert_eq!(launched(&mut app), [Launch::Print(vec![FsPath::parse(r"C:\x\a.pdf").unwrap()])]);
    let iso = right_click(&mut app, Some(3), false);
    assert!(!commands(&iso.menu).iter().any(|l| l == "Print"));
    run(&mut app, &iso, "Mount disk image");
    assert_eq!(launched(&mut app), [Launch::Mount(FsPath::parse(r"C:\x\disk.iso").unwrap())]);
    assert!(commands(&right_click(&mut app, Some(4), false).menu).iter().any(|l| l == "Install font"), "fonts install on Linux too");
    assert!(commands(&right_click(&mut app, None, false).menu).iter().any(|l| l == "Open in system file manager"));
}

#[test]
fn windows_menus_keep_their_shell_verbs_instead_of_the_linux_entries() {
    let mut app = app_with(&[("sub", true), ("a.pdf", false), ("b.txt", false), ("disk.iso", false)]);
    let shown: Vec<String> = [Some(1), Some(3), None].into_iter().flat_map(|row| commands(&right_click(&mut app, row, false).menu)).collect();
    for gone in ["Print", "Mount disk image", "Open in system file manager"] {
        assert!(!shown.iter().any(|l| l == gone), "{gone} in {shown:?}");
    }
    let mut folder = app_with(&[("sub", true), ("a.pdf", false)]);
    folder.caps = crate::caps::Caps { unix_paths: true };
    let menu = right_click(&mut folder, Some(0), false);
    assert!(!commands(&menu.menu).iter().any(|l| l == "Print"), "a folder is not printed");
}
