use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::menu::MenuItem;
use crate::ui::settings::{self, Kind, Page, Value, SETTINGS};
use crate::ui::{harness, ShellMenu};

fn shell_menu(a: &mut App) -> ShellMenu {
    a.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).expect("a menu was asked for")
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Command { label, disabled: true, .. } => format!("({label})"),
            MenuItem::Command { label, .. } => label.clone(),
            MenuItem::Submenu { label, .. } => format!("{label} >"),
            MenuItem::Header(h) => h.clone(),
            MenuItem::Separator => "-".to_owned(),
        })
        .collect()
}

fn title(id: &str) -> String {
    ACTIONS.iter().find(|a| a.id == id).unwrap().title.t().to_owned()
}

fn key(a: &mut App, key: Key, mods: Mods) {
    a.on_event(&Event::Key { key, mods, pressed: true }, Instant::now());
    harness::frame(a);
}

fn show(a: &mut App, m: ShellMenu) {
    a.on_event(&Event::ShowMenu { items: m.menu, x: m.x, y: m.y, launches: Vec::new(), kind: m.kind, shell: Vec::new() }, Instant::now());
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
}

#[test]
fn f10_opens_the_main_menu_with_its_commands_and_flit_window_items() {
    let mut a = harness::app(&["a.txt"]);
    key(&mut a, Key::F(10), Mods::default());
    let m = shell_menu(&mut a);
    assert_eq!(m.kind, MenuKind::None);
    let l = labels(&m.menu);
    assert_eq!(l[..7], [title("goto.open"), title("palette.show"), "-".into(), format!("({})", title("layout.load")), title("layout.save"), title("options.open"), "-".into()]);
    assert_eq!(l[7..], [S::MenuNewWindow.t(), S::MenuMiniWindow.t(), S::MenuFullScreen.t()]);
    assert!(a.menu_open_from(Hit::Menu) || a.menu_owner == Some(Hit::Menu));
    a.set_layouts(vec!["work".into()]);
    a.open_main_menu();
    assert_eq!(labels(&shell_menu(&mut a).menu)[3], title("layout.load"), "Load Layout is enabled once a layout exists");
}

#[test]
fn the_panel_menu_lists_the_quick_group_in_sections_and_opens_at_a_middle_click_on_empty_space() {
    let mut a = harness::app(&["a.txt"]);
    a.panel_quick_menu();
    let m = shell_menu(&mut a);
    let l = labels(&m.menu);
    let quick: Vec<String> = ACTIONS.iter().filter(|x| x.group == Group::Quick).map(|x| x.title.t().to_owned()).collect();
    assert_eq!(l.iter().filter(|x| *x != "-").cloned().collect::<Vec<_>>(), quick);
    assert!(l.iter().filter(|x| *x == "-").count() >= 4);
    let r = a.layout.list;
    let (x, y) = (r.x + 120.0, r.bottom() - 20.0);
    a.on_event(&Event::MouseDown { x, y, button: MouseButton::Middle, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseUp { x, y, button: MouseButton::Middle }, Instant::now());
    let m = shell_menu(&mut a);
    assert_eq!((m.x, m.y), (x, y), "the menu opens at the cursor");
    assert_eq!(labels(&m.menu).iter().filter(|x| *x != "-").count(), quick.len());
}

#[test]
fn the_background_menu_starts_with_own_view_sort_and_group_submenus() {
    let mut a = harness::app(&["a.txt"]);
    let r = a.layout.list;
    let (x, y) = (r.x + 120.0, r.bottom() - 20.0);
    a.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, Instant::now());
    let m = shell_menu(&mut a);
    assert_eq!(m.kind, MenuKind::Background);
    assert_eq!(labels(&m.menu)[..3], [format!("{} >", S::MenusView.t()), format!("{} >", S::MenusSort.t()), format!("{} >", S::MenusGroup.t())]);
    let MenuItem::Submenu { items, .. } = &m.menu[1] else { panic!() };
    assert!(items.iter().any(|i| matches!(i, MenuItem::Command { mark: crate::ui::Mark::Sort(_), checked: true, .. })), "the sorted column is marked with its direction");
}

#[test]
fn pinning_asks_first_and_the_pins_persist_per_menu_kind() {
    let mut a = harness::app(&["a.txt"]);
    let menu = vec![MenuItem::command("Open", String::new()), MenuItem::command("Copy", String::new())];
    let dir = a.tab().view_loc.path().clone();
    show(&mut a, ShellMenu { dir, items: Vec::new(), x: 200.0, y: 150.0, extended: false, menu, inline: false, shell: false, kind: MenuKind::Folder });
    let c = a.context_menu_open().expect("folder menus are own context menus");
    let star = c.star_rect(c.rects[1]);
    let (x, y) = (star.x + star.w / 2.0, star.y + star.h / 2.0);
    for ev in [Event::MouseMove { x, y }, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Event::MouseUp { x, y, button: MouseButton::Left }] {
        a.on_event(&ev, Instant::now());
    }
    let ask = a.confirm.clone().expect("pinning asks first");
    assert_eq!((ask.question.as_str(), ask.yes.as_str()), (S::MenusPinQuestion.t(), S::MenusPinYes.t()));
    let b = ask.buttons[0];
    let (x, y) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
    for ev in [Event::MouseMove { x, y }, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Event::MouseUp { x, y, button: MouseButton::Left }] {
        a.on_event(&ev, Instant::now());
    }
    assert_eq!(a.opts.pins, [Vec::<String>::new(), vec!["Copy".to_owned()], Vec::new()]);
    assert_eq!(a.context_menu_open().unwrap().labels()[0], "* Copy");
    let k = settings::index("menu.pins_folder").unwrap();
    let text = settings::render(&a.setting_values(), &Default::default(), None);
    let doc = settings::parse(&text);
    assert!(doc.values.contains(&(k, Value::Strings(vec!["Copy".into()]))));
    let mut b = harness::app(&[]);
    b.apply_doc(&doc);
    assert_eq!(b.opts.pins[1], ["Copy"]);
    a.set_pin(MenuKind::Folder, "Copy".into(), false);
    assert!(a.opts.pins[1].is_empty());
}

#[test]
fn options_list_every_group_in_order_with_every_setting_and_search_highlights_matches() {
    let mut a = harness::app(&["a.txt"]);
    a.open_settings(None);
    harness::frame(&mut a);
    let lines = a.settings_lines();
    let groups: Vec<&str> = Page::ALL.iter().map(|p| p.title()).collect();
    let shown: Vec<&str> = lines.iter().map(String::as_str).filter(|l| groups.contains(l)).collect();
    assert_eq!(shown, groups);
    for s in SETTINGS.iter().filter(|s| !matches!(s.kind, Kind::Keys(..) | Kind::Strings(..))) {
        assert!(lines.iter().any(|l| l == s.id || *l == format!("off {}", s.id)), "{} is listed", s.id);
    }
    let at = |id: &str| lines.iter().position(|l| l == id || *l == format!("off {id}")).unwrap();
    assert!(at("appearance.font") < at("appearance.scale") && at("appearance.scale") < at(settings::SPACING) && at("appearance.system_scheme") < at("appearance.color_scheme"));
    assert!(at("files.show_hidden") < at("files.system") && at("files.auto_sizes") < at("files.sizes_ssd") && at("input.act_on_press") < at("input.char_key"));
    a.settings_insert("thumbnails");
    harness::frame(&mut a);
    let found = a.settings_lines();
    assert!(found.contains(&"files.thumbnails".to_owned()), "{found:?}");
    assert!(!found.contains(&"appearance.font".to_owned()));
    let k = settings::index("files.thumbnails").unwrap();
    assert!(!super::super::settings_page::option_matches("zzzq", &SETTINGS[k]));
    a.close_settings();
    a.open_settings(Some(Page::Input));
    harness::frame(&mut a);
    assert_eq!(a.settings_page(), Some(Page::Input), "opening at a group scrolls to it");
}

#[test]
fn option_rows_toggle_step_slide_and_pick() {
    let mut a = harness::app(&["a.txt"]);
    let set = |a: &mut App, id: &str, v: Value| assert!(SETTINGS[settings::index(id).unwrap()].set(a, v));
    set(&mut a, "input.reverse_scroll", Value::Bool(true));
    assert!(a.opts.reverse_scroll);
    set(&mut a, settings::SPACING, Value::Number(80));
    assert_eq!(a.opts.spacing, 80);
    set(&mut a, "files.thumbnails", Value::Choice(2));
    assert!(!a.prefs.thumbnails);
    set(&mut a, "files.thumbnails", Value::Choice(0));
    assert!(a.prefs.thumbnails && a.opts.thumbs == Thumbs::Local);
    set(&mut a, "appearance.inspector_font", Value::Text("Courier New".into()));
    assert_eq!(a.opts.inspector_font, "Courier New");
    set(&mut a, "appearance.system_scheme", Value::Bool(false));
    set(&mut a, "appearance.color_scheme", Value::Choice(1));
    assert_eq!(a.look.mode, Mode::Light);
    set(&mut a, "appearance.system_scheme", Value::Bool(true));
    assert_eq!((a.look.mode, a.scheme()), (Mode::System, Mode::Light));
    assert!(!SETTINGS[settings::index(settings::LONG_PATHS).unwrap()].enabled(&a), "long paths are always on in Flit");
}

#[test]
fn the_old_theme_setting_migrates_into_the_scheme_switch_and_choice() {
    let doc = settings::parse("{\"appearance.theme\": \"light\", \"schema\": 3}");
    assert_eq!(doc.value("appearance.system_scheme"), Some(&Value::Bool(false)));
    assert_eq!(doc.value("appearance.color_scheme"), Some(&Value::Choice(1)));
    let doc = settings::parse("{\"appearance.theme\": \"system\", \"schema\": 3}");
    assert_eq!(doc.value("appearance.system_scheme"), Some(&Value::Bool(true)));
    let doc = settings::parse("{\"files.thumbnails\": false, \"schema\": 3}");
    assert_eq!(doc.value("files.thumbnails"), Some(&Value::Choice(2)));
}

#[test]
fn spacing_commands_change_row_height_within_bounds() {
    let mut a = harness::app(&["a.txt"]);
    let before = a.layout.row_h;
    a.run_action("spacing.increase", Instant::now());
    harness::frame(&mut a);
    assert_eq!(a.opts.spacing, SPACING_DEFAULT + SPACING_STEP as u8);
    assert!(a.layout.row_h > before);
    for _ in 0..20 {
        a.run_action("spacing.decrease", Instant::now());
    }
    harness::frame(&mut a);
    assert_eq!(a.opts.spacing, 0);
    assert!(a.layout.row_h < before);
}

#[test]
fn search_tabs_lists_open_tabs_with_paths_and_opens_the_chosen_one() {
    let mut a = harness::app(&["a.txt"]);
    a.run_action("tab.new", Instant::now());
    harness::frame(&mut a);
    assert_eq!(a.tabs.len(), 2);
    key(&mut a, Key::Char('A'), Mods { ctrl: true, shift: true, ..Mods::default() });
    let p = a.nav.palette.as_ref().expect("Ctrl+Shift+A opens Search Tabs");
    assert_eq!(p.pick, Pick::Tabs);
    assert_eq!(p.titles.len(), 2);
    assert_eq!(p.matches.len(), 2, "both tabs are listed");
    assert!(p.rows.len() >= 2 && p.rect.h > 2.0 * 46.0, "both rows fit: {:?}", p.rect);
    assert_eq!(p.placeholder.as_deref(), Some(crate::tr!(S::MenusSearchTabs, n = 2).as_str()));
    a.choose_tab_row(0, Instant::now());
    assert_eq!(a.cur, 0);
}

#[test]
fn a_saved_layout_restores_tabs_and_quick_access() {
    let mut a = harness::app(&["a.txt"]);
    a.save_layout("  work  ");
    let saved = a.drain_effects().find_map(|e| if let Effect::Layout(LayoutCmd::Save(n, l)) = e { Some((n, l)) } else { None }).expect("saving writes a layout file");
    assert_eq!(saved.0, "work");
    assert_eq!(a.layouts(), ["work"]);
    let mut b = harness::app(&[]);
    b.run_action("tab.new", Instant::now());
    b.apply_layout(&saved.1, Instant::now());
    assert_eq!(b.tabs.len(), saved.1.tabs.len());
    assert_eq!(b.saved_layout(), *saved.1);
    assert_eq!(clean_layout_name("a/b:c"), Some("abc".into()));
    assert_eq!(clean_layout_name(" . "), None);
    a.delete_layout("work");
    assert!(a.layouts().is_empty());
}

#[test]
fn create_config_exports_defaults_next_to_the_settings_file() {
    let mut a = harness::app(&[]);
    a.about.file = crate::FsPath::parse("C:\\Profile\\settings.json");
    a.run_action("config.create", Instant::now());
    let path = a.drain_effects().find_map(|e| if let Effect::Settings(SettingsCmd::Export { path, .. }) = e { Some(path) } else { None }).expect("an export is asked for");
    assert_eq!(path.display(), "C:\\Profile\\settings.default.json");
}
