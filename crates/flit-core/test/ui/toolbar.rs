use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::palette::Pick;
use crate::ui::{MenuItem, Persisted, ShellMenu};
use crate::FsPath;
use std::sync::Arc;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

fn app_in(path: &str, size: (u32, u32), items: &[&str]) -> App {
    let mut app = App::new(Persisted { path: Location::parse(path).unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, size, Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if n.contains('.') { 0 } else { attr::DIR }, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    frame(&mut app);
    app.drain_effects().for_each(drop);
    app
}

fn app_at(path: &str, items: &[&str]) -> App {
    app_in(path, (1000, 600), items)
}

fn frame(app: &mut App) {
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
}

fn ids(app: &App) -> Vec<String> {
    app.toolbar_items()
        .iter()
        .map(|i| match i {
            ToolItem::Button(id, _) => id.clone(),
            ToolItem::Separator => "|".into(),
        })
        .collect()
}

fn button(app: &App, id: &str) -> Rect {
    let items = app.toolbar_items();
    let k = items.iter().position(|i| matches!(i, ToolItem::Button(b, _) if b == id)).unwrap_or_else(|| panic!("{id} not in {:?}", ids(app)));
    app.toolbar.placed.iter().find(|(i, _)| *i == k).map(|&(_, r)| r).unwrap_or_else(|| panic!("{id} is not placed"))
}

fn click_at(app: &mut App, x: f32, y: f32, button: MouseButton) {
    app.on_event(&Event::MouseDown { x, y, button, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button }, Instant::now());
}

fn press(app: &mut App, id: &str) {
    let r = button(app, id);
    click_at(app, r.x + r.w / 2.0, r.y + r.h / 2.0, MouseButton::Left);
}

fn menu_of(app: &mut App) -> ShellMenu {
    app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).expect("a menu")
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, .. } => vec![label.clone()],
            MenuItem::Submenu { label, items } => std::iter::once(format!("[{label}]")).chain(labels(items)).collect(),
            MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn choose(app: &mut App, menu: &ShellMenu, label: &str) {
    let all = labels(&menu.menu).into_iter().filter(|l| !l.starts_with('[')).collect::<Vec<_>>();
    let k = all.iter().position(|l| l == label).unwrap_or_else(|| panic!("{label} not in {all:?}"));
    app.on_event(&Event::MenuCommand(k), Instant::now());
}

fn select(app: &mut App, name: &str) {
    let v = app.tab_mut().view.as_mut().unwrap();
    let i = (0..v.listing().len() as u32).find(|&i| v.listing().name(i) == name.as_bytes()).unwrap();
    v.clear_selection();
    v.set_selected(i, true);
    frame(app);
}

fn palette_titles(app: &App) -> Vec<String> {
    app.nav.palette.as_ref().map(|p| p.titles.clone()).unwrap_or_default()
}

fn pick_row(app: &mut App, title: &str) {
    let k = palette_titles(app).iter().position(|t| t == title).unwrap_or_else(|| panic!("{title} not in {:?}", palette_titles(app)));
    app.run_palette_item(k, Instant::now());
}

#[test]
fn folders_get_the_main_group_and_the_view_menus_under_the_address_bar() {
    let app = app_at(r"C:\x", &["a.txt"]);
    assert_eq!(
        ids(&app),
        ["menu.new", "|", "file.cut", "file.copy", "file.paste", "file.rename", "file.share", "item.trash", "|", "item.properties", "|", "menu.select", "menu.sort", "menu.group", "menu.layout", "|", "view.pane"]
    );
    let l = &app.layout;
    assert_eq!(l.commands.y, l.toolbar.bottom());
    assert_eq!(l.commands.h, BAR_H);
    assert!(l.header.y >= l.commands.bottom());
    assert!(button(&app, "menu.new").w > button(&app, "file.copy").w, "menus show a label, actions only an icon");
}

#[test]
fn ctrl_shift_b_hides_and_shows_the_toolbar() {
    let mut app = app_at(r"C:\x", &["a.txt"]);
    let header = app.layout.header.y;
    app.on_event(&Event::Key { key: Key::Char('B'), mods: CTRL_SHIFT, pressed: true }, Instant::now());
    assert!(!app.toolbar_state().visible);
    assert_eq!(app.layout.commands.h, 0.0);
    assert_eq!(app.layout.header.y, header - BAR_H);
    app.on_event(&Event::Key { key: Key::Char('B'), mods: CTRL_SHIFT, pressed: true }, Instant::now());
    assert!(app.toolbar_state().visible);
    assert_eq!(app.layout.header.y, header);
}

#[test]
fn buttons_run_their_actions_and_follow_the_selection() {
    let mut app = app_at(r"C:\x", &["a.txt", "b.txt"]);
    assert!(!app.tool_enabled("file.copy"));
    press(&mut app, "file.copy");
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::SetClipboardFiles { .. })), "disabled without a selection");
    select(&mut app, "b.txt");
    press(&mut app, "file.copy");
    let copied = app.drain_effects().find_map(|e| if let Effect::SetClipboardFiles { paths, cut } = e { Some((paths, cut)) } else { None });
    assert_eq!(copied, Some((vec![FsPath::parse(r"C:\x\b.txt").unwrap()], false)));
    press(&mut app, "file.share");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Share(p) if p == [FsPath::parse(r"C:\x\b.txt").unwrap()])));
}

#[test]
fn new_select_sort_and_group_open_their_menus() {
    let mut app = app_at(r"C:\x", &["a.txt"]);
    press(&mut app, "menu.new");
    assert!(labels(&menu_of(&mut app).menu).contains(&"Folder".to_owned()));
    press(&mut app, "menu.select");
    let m = menu_of(&mut app);
    assert_eq!(labels(&m.menu), ["Select all", "Invert selection", "Clear selection"]);
    choose(&mut app, &m, "Select all");
    assert_eq!(app.tab().view.as_ref().unwrap().selected_count(), 1);
    press(&mut app, "menu.sort");
    let m = menu_of(&mut app);
    choose(&mut app, &m, "Size");
    assert_eq!(app.tab().view_spec.sort.field, crate::sort::SortField::Size);
    press(&mut app, "menu.group");
    assert!(labels(&menu_of(&mut app).menu).contains(&"Type".to_owned()));
}

#[test]
fn layout_flyout_switches_views_resizes_items_and_toggles_options() {
    let mut app = app_at(r"C:\x", &["a.txt"]);
    press(&mut app, "menu.layout");
    frame(&mut app);
    assert!(app.tool_checked("menu.layout"));
    let (row, slider, hidden) = {
        let f = app.toolbar.fly.as_ref().unwrap();
        (f.rows[1], f.slider, f.toggles[0])
    };
    click_at(&mut app, row.x + 20.0, row.y + row.h / 2.0, MouseButton::Left);
    assert_eq!(app.tab().mode, LayoutMode::List);
    let before = app.tab().sizes.level(app.tab().look);
    let y = slider.y + slider.h / 2.0;
    app.on_event(&Event::MouseDown { x: slider.x, y, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseMove { x: slider.right(), y }, Instant::now());
    app.on_event(&Event::MouseUp { x: slider.right(), y, button: MouseButton::Left }, Instant::now());
    let after = app.tab().sizes.level(app.tab().look);
    assert!(after > before && after + 1 == app.tab().look.levels() as usize, "{before} -> {after}");
    let shown = app.tab().view_spec.show_hidden;
    click_at(&mut app, hidden.x + 20.0, hidden.y + hidden.h / 2.0, MouseButton::Left);
    assert_eq!(app.tab().view_spec.show_hidden, !shown);
    assert!(app.toolbar.fly.is_some(), "toggles keep the flyout open");
    app.on_event(&Event::Key { key: Key::Escape, mods: NO, pressed: true }, Instant::now());
    assert!(app.toolbar.fly.is_none());
    assert_eq!(app.tab().view.as_ref().unwrap().selected_count(), 0);
}

#[test]
fn tooltips_name_the_button_and_its_shortcut_after_a_delay() {
    let mut app = app_at(r"C:\x", &["a.txt"]);
    let r = button(&app, "file.copy");
    let t0 = Instant::now();
    app.on_event(&Event::MouseMove { x: r.x + 4.0, y: r.y + 4.0 }, t0);
    assert_eq!(app.toolbar_tip(), None);
    app.on_event(&Event::Timer, t0 + TIP_DELAY + Duration::from_millis(20));
    assert_eq!(app.toolbar_tip().as_deref(), Some("Copy (Ctrl+C)"));
    let r = button(&app, "menu.layout");
    app.on_event(&Event::MouseMove { x: r.x + 4.0, y: r.y + 4.0 }, t0 + Duration::from_secs(1));
    assert_eq!(app.toolbar_tip().as_deref(), Some("Layout"), "the next tip shows at once");
}

#[test]
fn buttons_that_do_not_fit_move_into_more() {
    let mut app = app_in(r"C:\x", (420, 600), &["a.txt"]);
    frame(&mut app);
    assert!(!app.toolbar.overflow.is_empty());
    let more = app.toolbar.more;
    assert!(more.w > 0.0 && more.right() <= 420.0);
    click_at(&mut app, more.x + 4.0, more.y + 4.0, MouseButton::Left);
    let m = labels(&menu_of(&mut app).menu);
    assert!(m.contains(&"[Layout]".to_owned()) && m.contains(&"Details".to_owned()), "{m:?}");
}

#[test]
fn the_palette_adds_moves_restyles_removes_and_resets_buttons() {
    let mut app = app_at(r"C:\x", &["a.txt"]);
    app.run_action("toolbar.customize", Instant::now());
    assert!(matches!(&app.nav.palette, Some(p) if p.pick == Pick::Toolbar(ToolPick::Items(ToolContext::Folder))));
    pick_row(&mut app, "Add a button");
    pick_row(&mut app, "Refresh");
    assert_eq!(ids(&app).last().map(String::as_str), Some("nav.refresh"));
    let last = palette_titles(&app).iter().position(|t| t == "Refresh").unwrap();
    app.run_palette_item(last, Instant::now());
    pick_row(&mut app, "Move left");
    let n = ids(&app).len();
    assert_eq!(ids(&app)[n - 2], "nav.refresh");
    let k = palette_titles(&app).iter().position(|t| t == "Refresh").unwrap();
    app.run_palette_item(k, Instant::now());
    pick_row(&mut app, "Show the icon and the label");
    assert!(app.toolbar_items().contains(&ToolItem::Button("nav.refresh".into(), Show::Both)));
    let k = palette_titles(&app).iter().position(|t| t == "Refresh").unwrap();
    app.run_palette_item(k, Instant::now());
    pick_row(&mut app, "Remove");
    assert!(!ids(&app).contains(&"nav.refresh".to_owned()));
    pick_row(&mut app, "Add a separator");
    assert_eq!(ids(&app).last().map(String::as_str), Some("|"));
    pick_row(&mut app, "Reset these buttons to the defaults");
    assert_eq!(app.toolbar_items(), ToolContext::Folder.defaults());
    pick_row(&mut app, "Edit the toolbar of the Recycle Bin");
    assert!(matches!(&app.nav.palette, Some(p) if p.pick == Pick::Toolbar(ToolPick::Items(ToolContext::RecycleBin))));
    assert_eq!(palette_titles(&app)[..3], ["Restore", "Restore all", "Empty Recycle Bin"]);
}

#[test]
fn the_toolbar_context_menu_customizes_resets_and_hides() {
    let mut app = app_at(r"C:\x", &["a.txt"]);
    app.toolbar.state.sets[0].truncate(1);
    frame(&mut app);
    let r = app.layout.commands;
    click_at(&mut app, r.right() - 40.0, r.y + r.h / 2.0, MouseButton::Right);
    let m = menu_of(&mut app);
    assert_eq!(labels(&m.menu), ["Customize the toolbar", "Reset the toolbar to defaults", "Hide the toolbar"]);
    choose(&mut app, &m, "Reset the toolbar to defaults");
    assert_eq!(app.toolbar_items(), ToolContext::Folder.defaults());
    click_at(&mut app, r.right() - 40.0, r.y + r.h / 2.0, MouseButton::Right);
    let m = menu_of(&mut app);
    choose(&mut app, &m, "Hide the toolbar");
    assert!(!app.toolbar_state().visible);
}

#[test]
fn recycle_bin_this_pc_and_the_home_page_get_their_own_sets() {
    let app = app_at("Recycle Bin", &[]);
    assert_eq!(&ids(&app)[..3], ["bin.restore", "bin.restore_all", "bin.empty"]);
    let app = app_at("This PC", &[]);
    assert_eq!(ids(&app)[0], "item.properties");
    assert!(!ids(&app).contains(&"menu.select".to_owned()));
    let mut app = app_at(r"C:\x", &[]);
    app.navigate(Location::home_page(), Instant::now());
    frame(&mut app);
    assert_eq!(app.layout.commands.h, 0.0);
}

#[test]
fn archives_add_an_extract_group() {
    let mut app = app_at(r"C:\x", &["a.zip", "b.txt"]);
    assert!(!ids(&app).contains(&"menu.extract".to_owned()));
    select(&mut app, "a.zip");
    assert_eq!(ids(&app)[ids(&app).len() - 2..], ["|", "menu.extract"]);
    press(&mut app, "menu.extract");
    let m = labels(&menu_of(&mut app).menu);
    assert_eq!(m, ["Extract files…", "Extract here", "Smart extract", "Extract to a folder named after the archive", "Open as folder"]);
    select(&mut app, "b.txt");
    assert!(!ids(&app).contains(&"menu.extract".to_owned()));
}

#[test]
fn saved_states_drop_unknown_and_repeated_buttons() {
    let b = |id: &str| ToolItem::Button(id.into(), Show::Icon);
    let mut st = ToolbarState::default();
    st.sets[0] = vec![b("file.copy"), b("no.such"), ToolItem::Separator, b("file.copy")];
    st.sets[1] = (0..100).map(|_| ToolItem::Separator).collect();
    let st = st.sanitized();
    assert_eq!(st.sets[0], [b("file.copy"), ToolItem::Separator]);
    assert_eq!(st.sets[1].len(), MAX_ITEMS);
}
