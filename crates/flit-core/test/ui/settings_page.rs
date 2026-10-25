use super::*;
use crate::input::Event;
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::provider::FileOp;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::tags::{FileKey, TagStore, Tags, COLORS};
use crate::text::tests::Mono;
use crate::ui::settings::index;
use crate::ui::{ConfirmDelete, MenuItem, Persisted, Startup};
use std::sync::Arc;
use std::time::{Duration, Instant};

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };

fn app() -> App {
    let mut a = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1100, 720), Instant::now());
    a.drain_effects().for_each(drop);
    a
}

fn listed(items: &[(&str, bool)]) -> App {
    let mut a = app();
    let t = a.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, size: 1500, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    a.poll();
    a.drain_effects().for_each(drop);
    a
}

fn key(a: &mut App, key: Key, mods: Mods) {
    a.on_event(&Event::Key { key, mods, pressed: true }, Instant::now());
}

fn typed(a: &mut App, text: &str) {
    for c in text.chars() {
        a.on_event(&Event::Char(c), Instant::now());
    }
}

fn click_at(a: &mut App, (x, y): (f32, f32)) {
    a.on_event(&Event::MouseMove { x, y }, Instant::now());
    a.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
}

fn center(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn ui(a: &App) -> &SettingsUi {
    a.settings.as_ref().expect("settings are open")
}

fn line(a: &mut App, id: &str) -> usize {
    let i = index(id).unwrap();
    let k = ui(a).lines.iter().position(|l| *l == Line::Setting(i)).unwrap_or_else(|| panic!("{id} is not shown"));
    let u = a.settings.as_mut().unwrap();
    (u.focus, u.reveal) = (k, true);
    a.layout();
    k
}

fn rect(a: &App, k: usize) -> Rect {
    ui(a).rects[k]
}

fn s(v: f32) -> f32 {
    v.round()
}

fn value(a: &App, id: &str) -> Value {
    SETTINGS[index(id).unwrap()].get(a)
}

fn tags_of(names: &[&str]) -> Arc<Tags> {
    let mut store = TagStore { tags: Vec::new(), files: Default::default() };
    for (k, n) in names.iter().enumerate() {
        store.add(n, COLORS[k].1).unwrap();
    }
    store.update(FileKey { volume: 1, id: 1 }, FsPath::parse(r"C:\x\a.txt").unwrap(), |_| 1);
    Arc::new(Tags::new(1, &store, &Default::default()))
}

#[test]
fn opens_from_the_keyboard_the_palette_the_sidebar_and_the_tab_menu() {
    let mut a = app();
    key(&mut a, Key::Char(','), CTRL);
    assert_eq!(a.settings_page(), Some(Page::General));
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Settings(SettingsCmd::CacheSize))));
    key(&mut a, Key::Escape, NO);
    assert!(!a.settings_open());
    key(&mut a, Key::Char('P'), Mods { ctrl: true, shift: true, alt: false });
    typed(&mut a, "settings");
    key(&mut a, Key::Enter, NO);
    assert!(a.settings_open(), "the palette command opens the settings");
    key(&mut a, Key::Char(','), CTRL);
    assert!(!a.settings_open(), "Ctrl+, closes them again");
    let button = a.nav.sidebar.settings;
    assert!(button.w > 0.0 && button.bottom() <= a.size.1);
    click_at(&mut a, center(button));
    assert!(a.settings_open(), "the button at the bottom of the sidebar opens them");
    let at = (ui(&a).panel.x / 2.0, a.size.1 - 4.0);
    click_at(&mut a, at);
    assert!(!a.settings_open(), "a click outside the window closes it");
    let items = a.build_tab_actions_menu();
    assert!(items.iter().any(|i| matches!(i, MenuItem::Command { label, .. } if label == "Settings")));
    a.set_tags(tags_of(&["Work"]));
    let tag_menu = a.build_tag_menu(None);
    let k = tag_menu.iter().position(|i| matches!(i, MenuItem::Command { label, .. } if label == "Manage tags")).unwrap();
    a.menu_command(k, Instant::now());
    assert_eq!(a.settings_page(), Some(Page::Tags), "Manage tags in the sidebar opens the Tags page");
}

#[test]
fn search_finds_settings_on_every_page_and_jumps_to_the_found_one() {
    let mut a = app();
    a.open_settings(None);
    typed(&mut a, "hidden");
    let lines = a.settings_lines();
    assert!(lines.contains(&"Files and folders".to_owned()) && lines.contains(&"files.show_hidden".to_owned()), "{lines:?}");
    assert!(!lines.contains(&"general.startup".to_owned()));
    typed(&mut a, " zzzz");
    assert_eq!(a.settings_lines(), ["No settings match the search."]);
    key(&mut a, Key::Escape, NO);
    assert!(a.settings_open() && ui(&a).query().is_empty(), "Escape clears the search first");
    typed(&mut a, "export");
    assert!(a.settings_lines().iter().any(|l| l.contains("Export settings")));
    for _ in 0.."export".len() {
        key(&mut a, Key::Backspace, NO);
    }
    typed(&mut a, "decimal");
    key(&mut a, Key::Enter, NO);
    key(&mut a, Key::Enter, NO);
    assert_eq!(a.settings_page(), Some(Page::Files));
    assert!(ui(&a).query().is_empty());
    let focus = ui(&a).focus;
    assert_eq!(focus, line(&mut a, "files.size_units"), "Enter on a result opens its page with the setting focused");
}

#[test]
fn controls_change_settings_with_the_mouse_and_the_keyboard() {
    let mut a = app();
    a.open_settings(Some(Page::Files));
    let k = line(&mut a, "files.show_hidden");
    let at = center(switch_rect(rect(&a, k), s));
    click_at(&mut a, at);
    assert_eq!(value(&a, "files.show_hidden"), Value::Bool(true));
    assert!(a.tabs().iter().all(|t| t.view_spec.show_hidden));
    let k = line(&mut a, "files.confirm_delete");
    a.drain_effects().for_each(drop);
    let at = center(control_rect(rect(&a, k), s));
    click_at(&mut a, at);
    let menu = a.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).expect("a choice menu");
    assert_eq!(menu.len(), 3);
    assert!(matches!(&menu[1], MenuItem::Command { checked: true, .. }));
    a.on_event(&Event::MenuCommand(2), Instant::now());
    assert_eq!(a.confirm_delete, ConfirmDelete::Never);
    ui_focus(&mut a, k);
    key(&mut a, Key::Left, NO);
    assert_eq!(a.confirm_delete, ConfirmDelete::Permanent, "Left picks the previous choice");
    a.open_settings(Some(Page::Advanced));
    let k = line(&mut a, "advanced.thumb_cache_mb");
    let (minus, plus) = step_rects(rect(&a, k), s);
    click_at(&mut a, center(plus));
    assert_eq!(value(&a, "advanced.thumb_cache_mb"), Value::Number(600));
    click_at(&mut a, center(minus));
    click_at(&mut a, center(minus));
    assert_eq!(value(&a, "advanced.thumb_cache_mb"), Value::Number(400));
    ui_focus(&mut a, k);
    key(&mut a, Key::Right, NO);
    assert_eq!(a.prefs.thumb_cache_mb, 500);
    let k = line(&mut a, "advanced.flatten_in_menu");
    ui_focus(&mut a, k);
    key(&mut a, Key::Space, NO);
    assert!(a.show_flatten);
}

fn ui_focus(a: &mut App, k: usize) {
    let u = a.settings.as_mut().unwrap();
    (u.focus, u.searching) = (k, false);
}

#[test]
fn startup_pages_are_added_and_removed_in_the_list() {
    let mut a = app();
    a.open_settings(Some(Page::General));
    let i = index("general.startup_pages").unwrap();
    let buttons = ui(&a).lines.iter().position(|l| *l == Line::Buttons(Some(i), PATH_BUTTONS)).unwrap();
    let rects = button_rects(&ui(&a).lines[buttons].clone(), rect(&a, buttons), s);
    click_at(&mut a, center(rects[0]));
    let buttons = ui(&a).lines.iter().position(|l| *l == Line::Buttons(Some(i), PATH_BUTTONS)).unwrap();
    let rects = button_rects(&ui(&a).lines[buttons].clone(), rect(&a, buttons), s);
    click_at(&mut a, center(rects[1]));
    assert_eq!(a.startup_pages, [Location::home_page(), Location::parse(r"C:\x").unwrap()]);
    click_at(&mut a, center(rects[0]));
    assert_eq!(a.startup_pages.len(), 2, "a page is listed once");
    let buttons = ui(&a).lines.iter().position(|l| *l == Line::Buttons(Some(i), PATH_BUTTONS)).unwrap();
    let rects = button_rects(&ui(&a).lines[buttons].clone(), rect(&a, buttons), s);
    click_at(&mut a, center(rects[2]));
    typed(&mut a, r"D:\Projects");
    key(&mut a, Key::Enter, NO);
    assert_eq!(a.startup_pages.len(), 3);
    let first = ui(&a).lines.iter().position(|l| *l == Line::Path(i, 0)).unwrap();
    let at = center(remove_rect(rect(&a, first), s));
    click_at(&mut a, at);
    assert_eq!(a.startup_pages, [Location::parse(r"C:\x").unwrap(), Location::parse(r"D:\Projects").unwrap()]);
    a.choose_setting(index("general.startup").unwrap(), 2);
    assert_eq!(a.startup, Startup::Folders(a.startup_pages.clone()));
}

#[test]
fn advanced_page_exports_imports_resets_and_clears_the_cache() {
    let mut a = app();
    a.set_tags(tags_of(&["Work"]));
    a.open_settings(Some(Page::Advanced));
    let row = |a: &App, b: Btn| ui(a).lines.iter().position(|l| l.buttons().contains(&b)).unwrap();
    let press = |a: &mut App, b: Btn| {
        let k = row(a, b);
        let l = ui(a).lines[k].clone();
        let at = l.buttons().iter().position(|x| *x == b).unwrap();
        let r = button_rects(&l, rect(a, k), s)[at];
        click_at(a, center(r));
    };
    a.drain_effects().for_each(drop);
    press(&mut a, Btn::ClearCache);
    let fx: Vec<Effect> = a.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::ClearThumbnails)) && fx.iter().any(|e| matches!(e, Effect::Settings(SettingsCmd::CacheSize))));
    press(&mut a, Btn::Export);
    key(&mut a, Key::Enter, NO);
    let (path, text) = a.drain_effects().find_map(|e| if let Effect::Settings(SettingsCmd::Export { path, text, replace: false }) = e { Some((path, text)) } else { None }).expect("export");
    assert_eq!(path, FsPath::parse(r"C:\x\flit-settings.json").unwrap());
    assert!(text.contains("\"tags\": [{\"color\":\"#") && text.contains("\"name\":\"Work\""), "{text}");
    press(&mut a, Btn::Import);
    key(&mut a, Key::Enter, NO);
    let wanted = SettingsCmd::Import(FsPath::parse(r"C:\x\flit-settings.json").unwrap());
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Settings(c) if c == wanted)));
    a.single_click_files = true;
    press(&mut a, Btn::Reset);
    key(&mut a, Key::Enter, NO);
    assert!(!a.single_click_files);
    a.set_about(About { os: "Windows 11".into(), file: FsPath::parse(r"C:\Users\me\AppData\Local\Flit\settings.json"), cache_bytes: Some(3 << 20) });
    assert!(a.settings_lines().contains(&"Thumbnail cache in use: 3.0 MB".to_owned()));
    press(&mut a, Btn::OpenFile);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path.display().ends_with("settings.json"))));
}

#[test]
fn exporting_over_an_existing_file_asks_first_and_cancel_writes_nothing() {
    let mut a = app();
    a.open_settings(Some(Page::Advanced));
    let path = FsPath::parse(r"C:\x\notes.txt").unwrap();
    a.ask_replace_export(path.clone(), 1500, 0);
    assert_eq!(a.asking(), Some("replace"));
    assert_eq!(a.dialog_buttons(), ["Replace", "Cancel"]);
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    a.drain_effects().for_each(drop);
    key(&mut a, Key::Escape, NO);
    assert_eq!(a.asking(), None);
    assert!(a.settings_open(), "the dialog answers before the settings window");
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::Settings(SettingsCmd::Export { .. }))), "Cancel writes nothing");
    a.ask_replace_export(path.clone(), 1500, 0);
    key(&mut a, Key::Enter, NO);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Settings(SettingsCmd::Export { path: p, replace: true, .. }) if p == path)), "Replace writes over the file");
}

#[test]
fn tags_page_lists_tags_and_runs_their_commands() {
    let mut a = app();
    a.set_tags(tags_of(&["Work", "Home"]));
    a.open_settings(Some(Page::Tags));
    assert_eq!(a.settings_lines(), ["tag Work", "tag Home", "New tag"]);
    let r = button_rects(&Line::Tag(1), rect(&a, 1), s);
    click_at(&mut a, center(r[0]));
    assert!(matches!(a.nav.palette.as_ref().map(|p| &p.pick), Some(Pick::Tags(super::super::tags::TagPick::Name { tag: Some(1), .. }))));
    key(&mut a, Key::Escape, NO);
    assert!(a.settings_open(), "Escape closes the palette over the settings first");
    click_at(&mut a, center(r[2]));
    assert!(matches!(a.nav.palette.as_ref().map(|p| &p.pick), Some(Pick::Tags(super::super::tags::TagPick::Delete(1)))));
}

#[test]
fn about_page_shows_versions_and_libraries() {
    let mut a = app();
    a.set_about(About { os: "Windows 11 Pro 24H2 (build 26100.1)".into(), file: None, cache_bytes: None });
    a.open_settings(Some(Page::About));
    let lines = a.settings_lines();
    assert!(lines[0].starts_with("Version: Flit ") && lines[1] == "Windows: Windows 11 Pro 24H2 (build 26100.1)");
    assert!(lines.contains(&"serde_json: settings file".to_owned()));
}

#[test]
fn appearance_page_hides_the_toolbar_and_opens_its_customization() {
    let mut a = listed(&[("a.txt", false)]);
    a.open_settings(Some(Page::Appearance));
    assert_eq!(a.settings_lines()[..2], ["appearance.toolbar", "Customize the toolbar…"]);
    let k = line(&mut a, "appearance.toolbar");
    let at = center(switch_rect(rect(&a, k), s));
    click_at(&mut a, at);
    assert!(!a.toolbar.state.visible);
    ui_focus(&mut a, k);
    key(&mut a, Key::Right, NO);
    assert!(a.toolbar.state.visible);
    let l = ui(&a).lines[1].clone();
    let r = button_rects(&l, rect(&a, 1), s)[0];
    click_at(&mut a, center(r));
    assert!(matches!(a.nav.palette.as_ref().map(|p| &p.pick), Some(Pick::Toolbar(_))), "the customization layer opens over the settings");
    key(&mut a, Key::Escape, NO);
    assert!(a.settings_open());
    a.drain_effects().for_each(drop);
    assert!(SETTINGS[index("files.streams").unwrap()].set(&mut a, Value::Bool(true)));
    assert!(a.prefs.show_streams);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. }, .. })), "showing streams reads the folder again");
}

#[test]
fn the_window_draws_at_any_size_in_both_themes() {
    for (size, dark) in [((1100, 720), false), ((1100, 720), true), ((360, 260), false), ((2560, 1400), true)] {
        let mut a = app();
        a.set_tags(tags_of(&["Work"]));
        a.on_event(&Event::Resized { width: size.0, height: size.1 }, Instant::now());
        a.on_event(&Event::ThemeChanged { dark }, Instant::now());
        for page in Page::ALL {
            a.open_settings(Some(page));
            let mut g = Gfx::new(Mono, 1024);
            a.frame(&mut g, Instant::now());
            assert!(g.dl.instances.len() > 50);
        }
        a.on_event(&Event::Wheel { dx: 0.0, dy: -50.0 }, Instant::now());
        typed(&mut a, "a");
        a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    }
}

#[test]
fn delete_confirmation_follows_the_setting() {
    let mut a = listed(&[("a.txt", false), ("b.txt", false)]);
    key(&mut a, Key::Down, NO);
    a.trash_selection();
    assert_eq!(a.asking(), None, "moving to the Recycle Bin does not ask by default");
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { op: FileOp::Trash(_), .. }, .. })));
    a.confirm_delete = ConfirmDelete::Always;
    a.trash_selection();
    assert_eq!(a.asking(), Some("confirm"));
    a.jobs.all = true;
    key(&mut a, Key::Enter, NO);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { op: FileOp::Delete(_), .. }, .. })), "the check box deletes permanently instead");
    a.confirm_delete = ConfirmDelete::Never;
    key(&mut a, Key::Delete, SHIFT);
    assert_eq!(a.asking(), None);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { op: FileOp::Delete(_), .. }, .. })));
}

#[test]
fn folders_open_in_background_tabs_and_with_one_click_when_asked() {
    let mut a = listed(&[("docs", true), ("a.txt", false)]);
    a.switch_to_new_tab = false;
    key(&mut a, Key::Down, NO);
    key(&mut a, Key::Enter, CTRL);
    assert_eq!((a.tabs().len(), a.cur), (2, 0), "the new tab opens in the background");
    a.switch_to_new_tab = true;
    key(&mut a, Key::Enter, CTRL);
    assert_eq!((a.tabs().len(), a.cur), (3, 2));
    let mut a = listed(&[("docs", true), ("a.txt", false)]);
    a.single_click_folders = true;
    a.click_row(0, MouseButton::Left, 1, Instant::now());
    assert_eq!(a.current_location(), &Location::parse(r"C:\x\docs").unwrap());
    let mut a = listed(&[("docs", true), ("a.txt", false)]);
    a.single_click_files = true;
    a.click_row(0, MouseButton::Left, 1, Instant::now());
    assert_eq!(a.current_location(), &Location::parse(r"C:\x").unwrap(), "folders still need a double click");
    a.click_row(1, MouseButton::Left, 1, Instant::now());
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { .. }, .. })));
}

#[test]
fn sizes_dates_scrolling_and_menus_follow_their_settings() {
    let mut a = listed(&[("docs", true), ("a.txt", false)]);
    let arena = bumpalo::Bump::new();
    let v = a.tab().view.as_ref().unwrap();
    let file = (0..v.listing().len() as u32).find(|&i| !v.listing().entries[i as usize].is_dir()).unwrap();
    assert_eq!(a.cell_text(&arena, v.listing(), file, crate::ui::Column::Size), "1.5 KB");
    a.choose_setting(index("files.size_units").unwrap(), 1);
    let v = a.tab().view.as_ref().unwrap();
    assert_eq!(a.cell_text(&arena, v.listing(), file, crate::ui::Column::Size), "1.5 kB");
    a.smooth_scroll = false;
    a.tab_mut().scroll_target = 40.0;
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert_eq!(a.tab().scroll, a.tab().scroll_target, "without smooth scrolling the list jumps");
    key(&mut a, Key::Home, NO);
    let has = |items: &[MenuItem], label: &str| items.iter().any(|i| matches!(i, MenuItem::Command { label: l, .. } | MenuItem::Submenu { label: l, .. } if l == label));
    let menu = a.build_menu(true);
    assert!(!has(&menu, "Flatten folder"));
    a.show_flatten = true;
    assert!(has(&a.build_menu(true), "Flatten folder"));
    a.menu_hidden.push("item.open_tab");
    assert!(!has(&a.build_menu(true), "Open in new tab"));
}

#[test]
#[ignore]
fn bench_settings_window() {
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let mut a = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (1400, 900), Instant::now());
    a.set_tags(tags_of(&["Work", "Home", "Old"]));
    let mut gfx = Gfx::new(Mono, 1024);
    a.frame(&mut gfx, Instant::now());
    let t = Instant::now();
    key(&mut a, Key::Char(','), CTRL);
    a.frame(&mut gfx, Instant::now());
    let open = t.elapsed();
    let mut typing = Vec::new();
    for _ in 0..20 {
        for c in "thumbnail".chars() {
            let t = Instant::now();
            a.on_event(&Event::Char(c), Instant::now());
            a.frame(&mut gfx, Instant::now());
            typing.push(t.elapsed());
        }
        key(&mut a, Key::Escape, NO);
    }
    typing.sort();
    let mut frames = Vec::new();
    for k in 0..400 {
        a.on_event(&Event::MouseMove { x: 500.0 + (k % 40) as f32 * 15.0, y: 120.0 + (k % 60) as f32 * 12.0 }, Instant::now());
        let t = Instant::now();
        a.frame(&mut gfx, Instant::now());
        frames.push(t.elapsed());
    }
    frames.sort();
    let text = settings::render(settings::defaults(), &Default::default(), None);
    let t = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(settings::parse(&text));
    }
    let parse = t.elapsed() / 1000;
    let doc = settings::parse(&text);
    let t = Instant::now();
    for _ in 0..1000 {
        a.apply_doc(&doc);
    }
    let apply = t.elapsed() / 1000;
    let t = Instant::now();
    for _ in 0..10_000 {
        std::hint::black_box(a.setting_values());
    }
    let values = t.elapsed() / 10_000;
    eprintln!(
        "BENCH settings: Ctrl+, to finished frame {:.3} ms; keystroke in search to finished frame median {:.3} ms max {:.3} ms; hover frames median {:.3} ms p99 {:.3} ms; parse {} B file {:.1} us, apply {:.1} us, read all values {:.2} us",
        ms(open),
        ms(typing[typing.len() / 2]),
        ms(*typing.last().unwrap()),
        ms(frames[200]),
        ms(frames[396]),
        text.len(),
        parse.as_secs_f64() * 1e6,
        apply.as_secs_f64() * 1e6,
        values.as_secs_f64() * 1e6
    );
}
