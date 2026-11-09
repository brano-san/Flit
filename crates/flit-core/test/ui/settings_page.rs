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

#[test]
fn settings_wheel_glides_toward_the_target_over_frames() {
    let t0 = Instant::now();
    let mut a = app();
    a.open_settings(Some(Page::Interface));
    a.frame(&mut Gfx::new(Mono, 1024), t0);
    let list = a.settings.as_ref().unwrap().list;
    a.on_event(&Event::MouseMove { x: list.x + list.w / 2.0, y: list.y + list.h / 2.0 }, t0);
    a.on_event(&Event::Wheel { dx: 0.0, dy: -1.0 }, t0);
    let target = a.settings.as_ref().unwrap().scroll.target;
    assert!(target > 0.0);
    let mut seen = Vec::new();
    for k in 1..40 {
        a.frame(&mut Gfx::new(Mono, 1024), t0 + Duration::from_millis(16 * k));
        seen.push(a.settings.as_ref().unwrap().scroll.pos);
    }
    assert!(seen[0] > 0.0 && seen[0] < target, "the first frame moves part of the way: {seen:?}");
    assert!(seen.windows(2).all(|w| w[1] >= w[0]));
    assert_eq!(*seen.last().unwrap(), target);
    let up_from = *seen.last().unwrap();
    a.on_event(&Event::Wheel { dx: 0.0, dy: 1.0 }, t0 + Duration::from_secs(1));
    a.frame(&mut Gfx::new(Mono, 1024), t0 + Duration::from_millis(1016));
    assert!(a.settings.as_ref().unwrap().scroll.pos < up_from, "wheel up moves on its first frame");
}

#[test]
fn a_cut_description_shows_its_full_text_on_hover_and_hides_on_key_wheel_and_leave() {
    let t0 = Instant::now();
    let mut a = app();
    a.on_event(&Event::Resized { width: 760, height: 520 }, t0);
    a.open_settings(Some(Page::Interface));
    a.frame(&mut Gfx::new(Mono, 1024), t0);
    let (k, r) = *a.settings.as_ref().unwrap().cut.first().expect("a narrow window cuts some description");
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    a.on_event(&Event::MouseMove { x, y }, t0);
    assert_eq!(a.settings.as_ref().unwrap().tip, Some((k, (x, y))));
    let plain = { let mut g = Gfx::new(Mono, 1024); a.frame(&mut g, t0); g.dl.instances.len() };
    a.on_event(&Event::MouseMove { x: x + 1.0, y }, t0);
    assert_eq!(a.settings.as_ref().unwrap().tip, Some((k, (x, y))), "the tip stays put while the pointer stays on the same text");
    key(&mut a, Key::Down, NO);
    assert_eq!(a.settings.as_ref().unwrap().tip, None);
    let hidden = { let mut g = Gfx::new(Mono, 1024); a.frame(&mut g, t0); g.dl.instances.len() };
    assert!(plain > hidden, "the tip draws a box and its text");
    a.on_event(&Event::MouseMove { x, y: y + 0.5 }, t0);
    assert!(a.settings.as_ref().unwrap().tip.is_some());
    a.on_event(&Event::Wheel { dx: 0.0, dy: -1.0 }, t0);
    assert_eq!(a.settings.as_ref().unwrap().tip, None);
    a.frame(&mut Gfx::new(Mono, 1024), t0 + Duration::from_secs(1));
    a.on_event(&Event::MouseMove { x, y }, t0);
    a.on_event(&Event::MouseLeave, t0);
    assert_eq!(a.settings.as_ref().unwrap().tip, None);
}

#[test]
fn tips_wrap_long_text_within_the_width() {
    let mut g = Gfx::new(Mono, 1024);
    let font = crate::text::FontStyle::new(crate::text::FontFamily::Ui, 12.0, false);
    let text = "one two three four five six seven eight nine ten eleven twelve";
    let max = g.measure("one two three four", font);
    let lines = wrap(&mut g, text, font, max);
    assert!(lines.len() > 2);
    assert!(lines.iter().all(|l| g.measure(l, font) <= max));
    assert_eq!(lines.join(" "), text);
    assert!(wrap(&mut g, "", font, max).is_empty());
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
    reveal(a, k)
}

fn reveal(a: &mut App, k: usize) -> usize {
    let u = a.settings.as_mut().unwrap();
    (u.focus, u.reveal) = (k, true);
    a.layout();
    let u = a.settings.as_mut().unwrap();
    u.scroll.snap(u.scroll.target);
    a.layout();
    k
}

fn page(a: &App, p: Page) -> Vec<String> {
    let lines = a.settings_lines();
    let titles: Vec<&str> = Page::ALL.iter().map(|p| p.title()).collect();
    let start = lines.iter().position(|l| l == p.title()).map_or(0, |k| k + 1);
    let end = lines[start..].iter().position(|l| titles.contains(&l.as_str())).map_or(lines.len(), |k| start + k);
    lines[start..end].to_vec()
}

fn path_buttons(a: &mut App, i: usize) -> Vec<Rect> {
    let k = ui(a).lines.iter().position(|l| *l == Line::Buttons(Some(i), PATH_BUTTONS)).unwrap();
    reveal(a, k);
    button_rects(&ui(a).lines[k].clone(), rect(a, k), &ui(a).fit, s)
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
    assert_eq!(a.settings_page(), Some(Page::Interface));
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Settings(SettingsCmd::CacheSize))));
    key(&mut a, Key::Escape, NO);
    assert!(!a.settings_open());
    key(&mut a, Key::Char('P'), Mods { ctrl: true, shift: true, alt: false });
    typed(&mut a, "options");
    key(&mut a, Key::Enter, NO);
    assert!(a.settings_open(), "the palette command opens the settings");
    key(&mut a, Key::Char(','), CTRL);
    assert!(!a.settings_open(), "Ctrl+, closes them again");
    let sb = &a.nav.sidebar;
    assert_eq!(sb.list.bottom(), sb.rect.bottom(), "Quick Access has no settings button at the bottom");
    key(&mut a, Key::Char(','), CTRL);
    assert!(a.settings_open());
    let at = (ui(&a).panel.x / 2.0, a.size.1 - 4.0);
    click_at(&mut a, at);
    assert!(!a.settings_open(), "a click outside the window closes it");
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
    assert!(lines.contains(&Page::Files.title().to_owned()) && lines.contains(&"files.show_hidden".to_owned()), "{lines:?}");
    assert!(!lines.contains(&"general.startup".to_owned()));
    typed(&mut a, " zzzz");
    assert_eq!(a.settings_lines(), [S::OptionsNoMatch.t()]);
    key(&mut a, Key::Escape, NO);
    assert!(a.settings_open() && ui(&a).query().is_empty(), "Escape clears the search first");
    typed(&mut a, "export");
    assert!(a.settings_lines().iter().any(|l| l.contains("Export settings")));
    for _ in 0.."export".len() {
        key(&mut a, Key::Backspace, NO);
    }
    typed(&mut a, "decimal");
    key(&mut a, Key::Enter, NO);
    a.drain_effects().for_each(drop);
    key(&mut a, Key::Enter, NO);
    assert_eq!(ui(&a).lines[ui(&a).focus], Line::Setting(index("files.size_units").unwrap()));
    assert!(a.drain_effects().any(|e| matches!(e, Effect::ShellMenu(_))), "Enter on a result uses it at once");
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
    let k = line(&mut a, settings::CACHE);
    let track = slider_track(rect(&a, k), s);
    click_at(&mut a, (track.right(), track.y + track.h / 2.0));
    assert_eq!(value(&a, settings::CACHE), Value::Number(crate::ui::THUMB_CACHE_MB.2 as i64), "the slider sets the value where it is clicked");
    click_at(&mut a, (track.x, track.y + track.h / 2.0));
    let low = crate::ui::THUMB_CACHE_MB.0 as i64;
    assert_eq!(value(&a, settings::CACHE), Value::Number(low));
    ui_focus(&mut a, k);
    key(&mut a, Key::Right, NO);
    assert_eq!(value(&a, settings::CACHE), Value::Number((low / 100 + 1) * 100));
    let k = line(&mut a, "appearance.scale");
    let (_, plus) = step_rects(rect(&a, k), s);
    click_at(&mut a, center(plus));
    assert_eq!(a.ui_scale(), 110, "the stepper adds a step");
    let k = line(&mut a, "advanced.flatten_in_menu");
    ui_focus(&mut a, k);
    key(&mut a, Key::Space, NO);
    assert!(a.show_flatten);
}

fn ui_focus(a: &mut App, k: usize) {
    let u = a.settings.as_mut().unwrap();
    (u.focus, u.zone) = (k, Zone::List);
}

#[test]
fn startup_pages_are_added_and_removed_in_the_list() {
    let mut a = app();
    a.open_settings(Some(Page::Windows));
    let i = index("general.startup_pages").unwrap();
    let rects = path_buttons(&mut a, i);
    click_at(&mut a, center(rects[0]));
    let rects = path_buttons(&mut a, i);
    click_at(&mut a, center(rects[1]));
    assert_eq!(a.startup_pages, [Location::home_page(), Location::parse(r"C:\x").unwrap()]);
    click_at(&mut a, center(rects[0]));
    assert_eq!(a.startup_pages.len(), 2, "a page is listed once");
    let rects = path_buttons(&mut a, i);
    click_at(&mut a, center(rects[2]));
    typed(&mut a, r"D:\Projects");
    key(&mut a, Key::Enter, NO);
    assert_eq!(a.startup_pages.len(), 3);
    let first = ui(&a).lines.iter().position(|l| *l == Line::Path(i, 0)).unwrap();
    reveal(&mut a, first);
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
        let r = button_rects(&l, rect(a, k), &ui(a).fit, s)[at];
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
    a.single_click_files = crate::ui::settings::ClickOpen::Always;
    press(&mut a, Btn::Reset);
    key(&mut a, Key::Enter, NO);
    assert_eq!(a.single_click_files, crate::ui::settings::ClickOpen::Never);
    a.set_about(About { os: "Windows 11".into(), file: FsPath::parse(r"C:\Users\me\AppData\Local\Flit\settings.json"), cache_bytes: Some(3 << 20), logs: None });
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
    assert_eq!(page(&a, Page::Tags), ["tag Work", "tag Home", "New tag"]);
    let k = ui(&a).lines.iter().position(|l| *l == Line::Tag(1)).unwrap();
    reveal(&mut a, k);
    let r = button_rects(&Line::Tag(1), rect(&a, k), &ui(&a).fit, s);
    click_at(&mut a, center(r[0]));
    assert!(matches!(a.nav.palette.as_ref().map(|p| &p.pick), Some(Pick::Tags(super::super::tags::TagPick::Name { tag: Some(1), .. }))));
    key(&mut a, Key::Escape, NO);
    assert!(a.settings_open(), "Escape closes the palette over the settings first");
    click_at(&mut a, center(r[2]));
    assert!(matches!(a.nav.palette.as_ref().map(|p| &p.pick), Some(Pick::Tags(super::super::tags::TagPick::Delete(1)))));
}

#[test]
fn about_page_shows_versions_and_no_libraries() {
    let mut a = app();
    a.set_about(About { os: "Windows 11 Pro 24H2 (build 26100.1)".into(), file: None, cache_bytes: None, logs: None });
    a.open_settings(Some(Page::About));
    let lines = page(&a, Page::About);
    assert!(lines[0].starts_with("Version: Flit ") && lines[1] == format!("{OS_NAME}: Windows 11 Pro 24H2 (build 26100.1)"));
    assert!(!lines.iter().any(|l| l.starts_with("serde_json")), "{lines:?}");
    assert_eq!(lines[3..6], ["Updates", crate::ui::settings::CHECK_UPDATES, "Check for updates"]);
    assert_eq!(lines[6..10], ["Help","Documentation | Report a bug | Request a feature", "GitHub repository | Privacy statement", NO_TELEMETRY.t()]);
    let k = ui(&a).lines.iter().position(|l| l.buttons().contains(&Btn::ReportBug)).unwrap();
    reveal(&mut a, k);
    let r = button_rects(&ui(&a).lines[k].clone(), rect(&a, k), &ui(&a).fit, s)[1];
    a.drain_effects().for_each(drop);
    click_at(&mut a, center(r));
    let wanted = crate::provider::Launch::Run { program: "https://github.com/brano-san/Flit/issues/new?labels=bug".into(), args: String::new(), dir: None };
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } if launch == wanted)));
}

#[test]
fn advanced_page_groups_the_log_level_folder_and_buttons() {
    let mut a = app();
    a.set_about(About { logs: FsPath::parse(r"C:\Users\me\AppData\Local\Flit\logs"), ..Default::default() });
    a.open_settings(Some(Page::Advanced));
    let lines = page(&a, Page::Advanced);
    let at = lines.iter().position(|l| l == "Log file").expect("a Log file group");
    assert_eq!(lines[at..], ["Log file", settings::LOG_LEVEL, r"Folder: C:\Users\me\AppData\Local\Flit\logs", "Open the log file | Open the log folder"]);
    assert_eq!(lines.iter().filter(|l| *l == settings::LOG_LEVEL).count(), 1);
    assert_eq!(a.log_level, settings::LOG_DEFAULT);
    let k = ui(&a).lines.iter().position(|l| l.buttons().contains(&Btn::OpenLogs)).unwrap();
    reveal(&mut a, k);
    let r = button_rects(&ui(&a).lines[k].clone(), rect(&a, k), &ui(&a).fit, s)[1];
    click_at(&mut a, center(r));
    assert!(!a.settings_open(), "the folder opens in a tab the settings window would hide");
    assert_eq!(a.current_location().display(), r"C:\Users\me\AppData\Local\Flit\logs");
}

#[test]
fn system_integration_starts_off_on_every_platform() {
    let id = settings::DEFAULT_MANAGER.to_owned();
    for unix_paths in [false, true] {
        let mut a = app();
        a.caps = crate::caps::Caps { unix_paths };
        a.open_settings(Some(Page::Advanced));
        let lines = a.settings_lines();
        assert!(lines.contains(&id) && !lines.iter().any(|l| l.contains("open_in_flit")), "{lines:?}");
        assert_eq!(value(&a, &id), Value::Bool(false));
        typed(&mut a, "Open/Save");
        assert!(!a.settings_lines().contains(&id));
    }
}

#[test]
fn appearance_page_has_no_toolbar_options() {
    let mut a = listed(&[("a.txt", false)]);
    a.open_settings(Some(Page::Interface));
    let lines = page(&a, Page::Interface);
    assert_eq!(lines[..6], ["appearance.font", "appearance.scale", settings::CONTENT_SCALE, "appearance.inspector_font", "appearance.inspector_font_size", settings::SPACING]);
    assert!(index("appearance.toolbar").is_none());
    assert!(!lines.iter().any(|l| l.contains("toolbar")), "{lines:?}");
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
    a.single_click_folders = crate::ui::settings::ClickOpen::Always;
    a.click_row(0, MouseButton::Left, 1, Instant::now());
    assert_eq!(a.current_location(), &Location::parse(r"C:\x\docs").unwrap());
    let mut a = listed(&[("docs", true), ("a.txt", false)]);
    a.single_click_files = crate::ui::settings::ClickOpen::Always;
    a.click_row(0, MouseButton::Left, 1, Instant::now());
    assert_eq!(a.current_location(), &Location::parse(r"C:\x").unwrap(), "folders still need a double click");
    a.click_row(1, MouseButton::Left, 1, Instant::now());
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { .. }, .. })));
    let mut a = listed(&[("docs", true), ("a.txt", false)]);
    a.single_click_folders = crate::ui::settings::ClickOpen::Touch;
    a.on_event(&Event::Pointer { touch: false }, Instant::now());
    a.click_row(0, MouseButton::Left, 1, Instant::now());
    assert_eq!(a.current_location(), &Location::parse(r"C:\x").unwrap(), "a mouse click only selects in touch mode");
    a.on_event(&Event::Pointer { touch: true }, Instant::now());
    a.click_row(0, MouseButton::Left, 1, Instant::now());
    assert_eq!(a.current_location(), &Location::parse(r"C:\x\docs").unwrap(), "a tap opens in touch mode");
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

#[test]
fn inactive_archive_settings_are_dimmed_skipped_by_the_keyboard_and_ignore_clicks() {
    use crate::ui::settings::NEEDS_BUILTIN_ARCHIVES;
    let mut a = app();
    a.open_settings(Some(Page::Files));
    let before = a.archive.prefs.compression.format;
    let lines = a.settings_lines();
    let off: Vec<usize> = (0..lines.len()).filter(|&k| lines[k].starts_with("off ")).collect();
    assert_eq!(off.len(), NEEDS_BUILTIN_ARCHIVES.len() + 1, "{lines:?}");
    assert!(off.iter().all(|&k| ui(&a).lines[k] != Line::Setting(index(&lines[k][4..]).unwrap()) && !ui(&a).lines[k].focusable()));
    for _ in 0..lines.len() + 4 {
        key(&mut a, Key::Down, NO);
        assert!(!off.contains(&ui(&a).focus), "focus landed on {}", lines[ui(&a).focus]);
    }
    for &k in &off {
        a.settings_click(SHit::Control(k), 0.0, 0.0);
        a.settings_click(SHit::Line(k), 0.0, 0.0);
    }
    assert!(a.menu.is_empty() && a.nav.palette.is_none() && a.archive.prefs.compression.format == before, "a click on an inactive row opens nothing");
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let k = off.iter().copied().find(|&k| lines[k] != format!("off {}", settings::LONG_PATHS)).unwrap();
    assert_eq!(a.setting_help(&ui(&a).lines[k]).split(' ').next(), Some("Inactive:"));
    a.apply_doc(&settings::parse(r#"{"files.archive_format": "7z"}"#));
    assert_eq!(a.archive.prefs.compression.format, crate::archive::Format::SevenZ, "the settings file still applies");
    a.builtin_archives = true;
    a.layout();
    assert_eq!(a.settings_lines().into_iter().filter(|l| l.starts_with("off ")).collect::<Vec<_>>(), [format!("off {}", settings::LONG_PATHS)]);
    assert!(NEEDS_BUILTIN_ARCHIVES.iter().all(|id| SETTINGS[index(id).unwrap()].enabled(&a)));
}


#[test]
fn options_have_no_shortcuts_group() {
    let mut a = app();
    a.open_settings(None);
    let lines = a.settings_lines();
    assert!(!lines.iter().any(|l| l == "Actions" || l.starts_with("action ")), "{lines:?}");
    assert!(!ui(&a).lines.iter().any(|l| l.buttons().iter().any(|b| b.label() == "Restore all shortcuts")));
    typed(&mut a, "ctrl+shift+t");
    assert!(!a.settings_lines().iter().any(|l| l.starts_with("action ")));
    key(&mut a, Key::Escape, NO);
    a.apply_doc(&settings::parse(r#"{"keys": {"view.hidden": ["Ctrl+G"]}}"#));
    assert_eq!(a.keymap.text_of("view.hidden"), "Ctrl+G", "saved shortcuts still load");
    assert!(a.setting_values().iter().any(|v| matches!(v, Value::Keys(_))));
}

fn spacing_track(a: &mut App) -> (usize, Rect) {
    let k = line(a, settings::SPACING);
    reveal(a, k);
    (k, slider_track(rect(a, k), s))
}

fn spacing(a: &App) -> i64 {
    match value(a, settings::SPACING) {
        Value::Number(n) => n,
        v => panic!("{v:?}"),
    }
}

#[test]
fn sliders_jump_on_press_follow_the_drag_and_stop_on_release() {
    let t0 = Instant::now();
    let mut a = app();
    a.open_settings(Some(Page::Interface));
    a.frame(&mut Gfx::new(Mono, 1024), t0);
    let (_, tr) = spacing_track(&mut a);
    let y = tr.y + tr.h / 2.0;
    a.on_event(&Event::MouseMove { x: tr.x + 2.0, y }, t0);
    a.on_event(&Event::MouseDown { x: tr.x + 2.0, y, button: MouseButton::Left, clicks: 1 }, t0);
    assert_eq!(spacing(&a), 0, "a press on the track jumps there");
    let mut seen = Vec::new();
    for k in 1..=10 {
        a.on_event(&Event::MouseMove { x: tr.x + tr.w * k as f32 / 10.0, y: y + 40.0 }, t0);
        seen.push(spacing(&a));
    }
    assert!(seen.windows(2).all(|w| w[1] >= w[0]) && seen[4] > 0 && seen[4] < 100, "the thumb follows the pointer even off the row: {seen:?}");
    assert_eq!(*seen.last().unwrap(), 100);
    a.on_event(&Event::MouseMove { x: tr.x + tr.w / 2.0, y }, t0);
    a.on_event(&Event::MouseUp { x: tr.x + tr.w / 2.0, y, button: MouseButton::Left }, t0);
    let released = spacing(&a);
    assert_eq!(released, 50);
    a.on_event(&Event::MouseMove { x: tr.x, y }, t0);
    assert_eq!(spacing(&a), released, "after release moves change nothing");
    key(&mut a, Key::Right, NO);
    assert_eq!(spacing(&a), 60, "the keyboard still steps the slider");
}

#[test]
fn the_scrollbar_drags_its_thumb_and_pages_on_the_track() {
    let t0 = Instant::now();
    let mut a = app();
    a.on_event(&Event::Resized { width: 1100, height: 520 }, t0);
    a.open_settings(Some(Page::Interface));
    a.frame(&mut Gfx::new(Mono, 1024), t0);
    let (list, content) = (ui(&a).list, ui(&a).content_h);
    assert!(content > list.h);
    let thumb = scroll_thumb(list, content, ui(&a).scroll.pos, s).unwrap();
    let (x, y) = (thumb.x + thumb.w / 2.0, thumb.y + thumb.h / 2.0);
    a.on_event(&Event::MouseMove { x, y }, t0);
    assert_eq!(a.hover, Hit::Settings(SHit::Thumb));
    a.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t0);
    a.on_event(&Event::MouseMove { x, y: y + (list.h - thumb.h) / 2.0 }, t0);
    let half = ui(&a).scroll.pos;
    assert!((half - (content - list.h) / 2.0).abs() < 2.0, "dragging the thumb halfway scrolls halfway: {half}");
    a.on_event(&Event::MouseMove { x, y: list.bottom() + 200.0 }, t0);
    assert_eq!(ui(&a).scroll.pos, content - list.h, "the end of the track is the end of the list");
    a.on_event(&Event::MouseUp { x, y: list.bottom() + 200.0, button: MouseButton::Left }, t0);
    a.on_event(&Event::MouseMove { x, y: list.y + 2.0 }, t0);
    assert_eq!(a.hover, Hit::Settings(SHit::Track));
    a.on_event(&Event::MouseDown { x, y: list.y + 2.0, button: MouseButton::Left, clicks: 1 }, t0);
    a.on_event(&Event::MouseUp { x, y: list.y + 2.0, button: MouseButton::Left }, t0);
    assert_eq!(ui(&a).scroll.target, content - 2.0 * list.h, "a click above the thumb pages up");
}

#[test]
fn rows_keep_labels_and_controls_apart_at_any_width() {
    for width in [520, 700, 900, 1400, 1920] {
        let mut a = app();
        a.on_event(&Event::Resized { width, height: 800 }, Instant::now());
        a.open_settings(None);
        a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
        let u = ui(&a);
        for (line, &r) in u.lines.iter().zip(&u.rects) {
            let (Line::Setting(i) | Line::Off(i)) = *line else { continue };
            let c = control_rect(r, s);
            let label_end = r.x + s(PAD) + s(indent(i)) + label_w(i, r, s);
            let control_x = if matches!(SETTINGS[i].kind, Kind::Toggle(..)) { switch_rect(r, s).x } else { c.x };
            assert!(label_end + s(PAD) <= control_x + 0.5, "{} overlaps its control at {width}", SETTINGS[i].id);
            assert!(c.w >= s(CONTROL_MIN) && c.right() <= r.right(), "{} control {c:?} in {r:?}", SETTINGS[i].id);
            assert!(label_w(i, r, s) > 40.0, "{} keeps room for its label at {width}", SETTINGS[i].id);
        }
    }
}

#[test]
fn choice_values_stay_inside_their_box() {
    let mut a = app();
    a.on_event(&Event::Resized { width: 700, height: 800 }, Instant::now());
    a.open_settings(Some(Page::Advanced));
    let k = line(&mut a, settings::LOG_LEVEL);
    reveal(&mut a, k);
    let mut g = Gfx::new(Mono, 1024);
    let before = g.dl.instances.len();
    let c = control_rect(rect(&a, k), s);
    let t = crate::theme::DARK;
    Paint::new(&mut g, &t, 1.0).picker(c, &"long value ".repeat(20), a.ui_font(13.0), 0.0, false, false);
    let glyphs: Vec<_> = g.dl.instances[before..].iter().filter(|i| i.kind == crate::render::kind::GLYPH).collect();
    assert!(!glyphs.is_empty());
    assert!(glyphs.iter().all(|i| i.rect[0] >= c.x && i.rect[0] + i.rect[2] <= c.right() + 0.5), "every glyph stays inside {c:?}");
}

#[test]
fn options_open_where_the_command_palette_does_and_stay_inside_the_window() {
    for (w, h) in [(1100, 720), (1400, 900), (800, 420), (700, 300)] {
        let mut a = app();
        a.on_event(&Event::Resized { width: w, height: h }, Instant::now());
        a.open_settings(None);
        a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
        let panel = ui(&a).panel;
        assert!(panel.y >= 0.0 && panel.bottom() <= a.size.1, "{panel:?} at {w}x{h}");
        a.close_settings();
        a.open_palette();
        a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
        let palette = a.nav.palette.as_ref().unwrap().rect;
        if h >= 720 {
            assert_eq!(panel.y, palette.y, "the same top as the commands at {w}x{h}");
            assert!(panel.y < a.s(super::super::TAB_H + super::super::TOOL_H + 45.0), "higher than before at {w}x{h}");
        }
    }
}


#[test]
fn a_switch_slides_its_knob_and_blends_its_track() {
    let t0 = Instant::now();
    let mut a = app();
    a.open_settings(Some(Page::Interface));
    a.frame(&mut Gfx::new(Mono, 1024), t0);
    let knob = |a: &App, i: usize, at: Instant| a.settings.as_ref().unwrap().knobs.iter().find(|k| k.0 == i).unwrap().1.value(at, KNOB);
    let i = a.settings.as_ref().unwrap().knobs.iter().map(|k| k.0).find(|&i| SETTINGS[i].id != "appearance.animations").expect("the page shows a switch");
    let on = knob(&a, i, t0) == 1.0;
    assert!(SETTINGS[i].set(&mut a, Value::Bool(!on)));
    let t = Instant::now();
    assert!(a.frame(&mut Gfx::new(Mono, 1024), t), "the knob moves");
    let mid = knob(&a, i, t + Duration::from_millis(50));
    assert!(mid > 0.0 && mid < 1.0, "{mid}");
    a.frame(&mut Gfx::new(Mono, 1024), t + Duration::from_millis(400));
    assert_eq!(knob(&a, i, t + Duration::from_millis(400)), if on { 0.0 } else { 1.0 });
    a.animations = false;
    assert!(SETTINGS[i].set(&mut a, Value::Bool(on)));
    a.frame(&mut Gfx::new(Mono, 1024), t + Duration::from_millis(410));
    assert_eq!(knob(&a, i, t + Duration::from_millis(410)), if on { 1.0 } else { 0.0 }, "without Animations it snaps");
}

struct Tall;

impl crate::text::TextBackend for Tall {
    fn shape(&mut self, text: &str, style: FontStyle, out: &mut crate::text::ShapedText) {
        let advance = (style.px() * 0.62).round();
        out.glyphs.clear();
        for (i, (b, c)) in text.char_indices().enumerate() {
            out.glyphs.push(crate::text::ShapedGlyph { font: 0, id: c as u16, x: i as f32 * advance, y: 0.0, cluster: b as u32 });
        }
        out.width = out.glyphs.len() as f32 * advance;
    }
    fn rasterize(&mut self, _: crate::text::GlyphKey, _: &mut crate::text::GlyphBitmap) -> bool {
        false
    }
    fn metrics(&mut self, style: FontStyle) -> crate::text::FontMetrics {
        crate::text::FontMetrics { ascent: style.px() * 1.25, descent: style.px() * 0.45 }
    }
}

fn check_layout<B: crate::text::TextBackend>(mut g: Gfx<B>, (w, h): (u32, u32), describe: bool) {
    let mut a = app();
    a.on_event(&Event::Resized { width: w, height: h }, Instant::now());
    a.opts.describe = describe;
    a.open_settings(Some(Page::Interface));
    a.frame(&mut g, Instant::now());
    a.frame(&mut g, Instant::now());
    let (mf, ms) = (g.metrics(a.ui_font(13.0)), g.metrics(a.ui_font(12.0)));
    let font = a.ui_font(13.0);
    let u = ui(&a);
    let at = format!("{w}x{h} describe {describe}");
    for (k, line) in u.lines.iter().enumerate() {
        let r = u.rects[k];
        if let Line::Setting(i) | Line::Off(i) = line {
            let c = match SETTINGS[*i].kind {
                Kind::Toggle(..) => switch_rect(r, s),
                _ => control_rect(r, s),
            };
            assert!(c.y - r.y >= 2.0 && r.bottom() - c.bottom() >= 2.0, "{at}: {} {c:?} has no gap inside {r:?}", SETTINGS[*i].id);
            if describe {
                let (title, help) = text_baselines(r, mf, ms, s);
                assert!(title - mf.ascent >= r.y && help + ms.descent <= r.bottom() + 0.5, "{at}: {} text leaves {r:?}", SETTINGS[*i].id);
                assert!(title + mf.descent + 2.0 <= help - ms.ascent, "{at}: {} title and help collide", SETTINGS[*i].id);
            } else {
                assert!(line_h(mf) + 4.0 <= r.h, "{at}: {} title taller than its row", SETTINGS[*i].id);
            }
        }
        let rects = button_rects(line, r, &u.fit, s);
        for (b, (br, btn)) in rects.iter().zip(line.buttons()).enumerate() {
            assert!(br.x >= r.x + s(PAD) - 0.5 && br.right() <= r.right() - s(PAD) + 0.5, "{at}: {btn:?} {br:?} leaves the row {r:?}");
            assert!(br.y >= r.y && br.bottom() <= r.bottom(), "{at}: {btn:?} {br:?} leaves the row {r:?}");
            assert!(g.measure(btn.label(), font) + s(16.0) + glyph_w(btn, s) <= br.w, "{at}: {btn:?} is cut");
            for other in &rects[b + 1..] {
                assert!(br.right() <= other.x || br.bottom() <= other.y, "{at}: {btn:?} overlaps the next button");
            }
        }
    }
}

#[test]
fn rows_and_buttons_fit_any_font_metrics() {
    for size in [(1100, 720), (760, 520), (600, 700), (1600, 900)] {
        for describe in [false, true] {
            check_layout(Gfx::new(Mono, 1024), size, describe);
            check_layout(Gfx::new(Tall, 1024), size, describe);
        }
    }
}

#[test]
fn help_buttons_wrap_to_a_second_row_in_a_narrow_window() {
    let mut a = app();
    a.on_event(&Event::Resized { width: 600, height: 700 }, Instant::now());
    a.open_settings(Some(Page::About));
    a.frame(&mut Gfx::new(Tall, 1024), Instant::now());
    let k = ui(&a).lines.iter().position(|l| l.buttons().contains(&Btn::RequestFeature)).unwrap();
    let r = button_rects(&ui(&a).lines[k].clone(), rect(&a, k), &ui(&a).fit, s);
    assert!(r[2].y > r[0].y, "the third button moves below: {r:?}");
    assert!(rect(&a, k).h >= r[2].bottom() - rect(&a, k).y, "the row grows with its buttons");
    let mut wide = app();
    wide.on_event(&Event::Resized { width: 1600, height: 900 }, Instant::now());
    wide.open_settings(Some(Page::About));
    wide.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let r = button_rects(&ui(&wide).lines[k].clone(), rect(&wide, k), &ui(&wide).fit, s);
    assert!(r.iter().all(|b| b.y == r[0].y), "a wide window keeps one row");
}

#[test]
fn tall_metrics_make_rows_taller_than_short_ones() {
    let height = |g: Gfx<Tall>| {
        let mut a = app();
        a.opts.describe = true;
        a.open_settings(Some(Page::Interface));
        let mut g = g;
        a.frame(&mut g, Instant::now());
        let k = ui(&a).lines.iter().position(|l| matches!(l, Line::Setting(_))).unwrap();
        rect(&a, k).h
    };
    let mono = {
        let mut a = app();
        a.opts.describe = true;
        a.open_settings(Some(Page::Interface));
        a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
        let k = ui(&a).lines.iter().position(|l| matches!(l, Line::Setting(_))).unwrap();
        rect(&a, k).h
    };
    assert!(height(Gfx::new(Tall, 1024)) > mono, "rows follow the measured line height");
}

fn wide() -> App {
    let mut a = app();
    a.on_event(&Event::Resized { width: 1400, height: 900 }, Instant::now());
    a
}

fn start_of(a: &App, page: Page) -> Start {
    *ui(a).starts.iter().find(|st| st.page == page).unwrap_or_else(|| panic!("{page:?} has no category"))
}

fn offset(a: &App, page: Page) -> f32 {
    let u = ui(a);
    u.rects[start_of(a, page).line].y - u.list.y + u.scroll.pos
}

fn scroll_to(a: &mut App, pos: f32) {
    a.settings.as_mut().unwrap().scroll.snap(pos);
    a.layout();
}

fn nav_item(a: &App, page: Page) -> Rect {
    let k = ui(a).starts.iter().position(|st| st.page == page).unwrap();
    ui(a).nav_rects[k]
}

fn click_nav(a: &mut App, page: Page) {
    let at = center(nav_item(a, page));
    click_at(a, at);
}

#[test]
fn the_categories_are_the_setting_groups_in_order() {
    let mut a = wide();
    a.open_settings(None);
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let u = ui(&a);
    assert_eq!(u.starts.iter().map(|st| st.page).collect::<Vec<_>>(), Page::ALL);
    assert!(u.starts.iter().all(|st| u.lines[st.line] == Line::Group(st.page.title())));
    assert_eq!(u.nav_rects.len(), Page::ALL.len());
    assert!(!u.rail && u.nav.w == u.fit.nav_w(s) && u.list.x == u.nav.right());
    assert!(u.fit.titles > 0.0 && (u.fit.titles + s(NAV_TEXT_ROOM) <= u.nav.w + 0.5 || u.nav.w == s(NAV_MAX)), "the longest title fits its column");
    assert!(u.list.w >= s(MIN_W), "the categories never take room from the settings");
    assert!(u.nav_rects.windows(2).all(|w| w[0].bottom() <= w[1].y) && u.nav_rects.last().unwrap().bottom() <= u.nav.bottom());
    assert_eq!(a.settings_page(), Some(Page::Interface));
}

#[test]
fn a_category_click_glides_to_its_section_and_lights_it() {
    let t0 = Instant::now();
    let mut a = wide();
    a.open_settings(None);
    a.frame(&mut Gfx::new(Mono, 1024), t0);
    let goal = offset(&a, Page::Files);
    click_nav(&mut a, Page::Files);
    assert_eq!(ui(&a).scroll.target, goal);
    assert_eq!(a.settings_page(), Some(Page::Files), "the clicked category lights at once");
    a.frame(&mut Gfx::new(Mono, 1024), t0 + Duration::from_millis(16));
    let first = ui(&a).scroll.pos;
    assert!(first > 0.0 && first < goal, "the list glides: {first} of {goal}");
    for k in 2..80 {
        a.frame(&mut Gfx::new(Mono, 1024), t0 + Duration::from_millis(16 * k));
    }
    let start = start_of(&a, Page::Files).line;
    assert_eq!(rect(&a, start).y, ui(&a).list.y, "the section title lands at the top");
    assert!(matches!(ui(&a).lines[ui(&a).focus], Line::Setting(i) if SETTINGS[i].page == Page::Files), "the keyboard continues in the section");
    click_nav(&mut a, Page::About);
    a.frame(&mut Gfx::new(Mono, 1024), t0 + Duration::from_secs(5));
    assert_eq!(a.settings_page(), Some(Page::About), "the last category stays lit although its section cannot reach the top");
    a.animations = false;
    click_nav(&mut a, Page::Colors);
    a.frame(&mut Gfx::new(Mono, 1024), t0 + Duration::from_secs(6));
    assert_eq!(ui(&a).scroll.pos, offset(&a, Page::Colors), "without animations the jump is instant");
    assert_eq!(rect(&a, start_of(&a, Page::Colors).line).y, ui(&a).list.y);
}

#[test]
fn the_category_at_the_top_of_the_list_is_lit_while_scrolling() {
    let mut a = wide();
    a.open_settings(None);
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let files = offset(&a, Page::Files);
    let data = offset(&a, Page::Data);
    for (pos, page) in [(0.0, Page::Interface), (files - 1.0, Page::Colors), (files, Page::Files), ((files + data) / 2.0, Page::Files), (data, Page::Data)] {
        scroll_to(&mut a, pos);
        assert_eq!(a.settings_page(), Some(page), "at {pos}");
    }
    let list = ui(&a).list;
    a.on_event(&Event::MouseMove { x: list.x + list.w / 2.0, y: list.y + list.h / 2.0 }, Instant::now());
    a.on_event(&Event::Wheel { dx: 0.0, dy: 30.0 }, Instant::now());
    assert_eq!(a.settings_page(), Some(Page::Interface), "the wheel moves the light too");
}

#[test]
fn a_search_keeps_only_categories_with_matches_and_counts_them() {
    let mut a = wide();
    a.open_settings(None);
    typed(&mut a, "hidden");
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let u = ui(&a);
    assert!(!u.starts.is_empty() && u.starts.len() < Page::ALL.len(), "{:?}", u.starts);
    assert_eq!(u.nav_rects.len(), u.starts.len());
    for (k, st) in u.starts.iter().enumerate() {
        let end = u.starts.get(k + 1).map_or(u.lines.len(), |n| n.line);
        let shown = u.lines[st.line..end].iter().filter(|l| matches!(l, Line::Setting(_) | Line::Off(_) | Line::Buttons(None, _))).count();
        assert_eq!(st.matches, shown, "{:?}", st.page);
        assert!(st.matches > 0);
    }
    assert!(start_of(&a, Page::Files).matches >= 1);
    let height = ui(&a).list.h;
    typed(&mut a, " zzzz");
    assert!(ui(&a).nav_rects.is_empty());
    assert_eq!(ui(&a).list.h, height, "the window keeps its height while typing");
    for _ in 0..5 {
        key(&mut a, Key::Backspace, NO);
    }
    let last = *ui(&a).starts.last().unwrap();
    click_nav(&mut a, last.page);
    assert_eq!(ui(&a).query(), "hidden", "a jump keeps the search");
    assert_eq!(a.settings_page(), Some(last.page));
}

#[test]
fn narrow_windows_fold_the_categories_to_icons_and_then_hide_them() {
    let mut seen = [false; 3];
    for width in (320..=1600).step_by(20) {
        let mut a = app();
        a.on_event(&Event::Resized { width, height: 700 }, Instant::now());
        a.open_settings(None);
        a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
        let u = ui(&a);
        let mode = match u.nav.w {
            w if w == u.fit.nav_w(s) => 0,
            w if w == s(RAIL_W) => 1,
            _ => 2,
        };
        seen[mode] = true;
        assert_eq!(u.rail, mode == 1);
        assert_eq!(u.nav_rects.is_empty(), mode == 2);
        let floor = [s(MIN_W), s(RAIL_CONTENT_MIN), 0.0][mode];
        assert!(u.list.w >= floor, "{width}: {} is squeezed below {floor}", u.list.w);
        assert!(u.panel.x >= 0.0 && u.panel.right() <= a.size.0 + 0.5, "{width}: {:?}", u.panel);
        let tip = a.tip_text(Hit::Settings(SHit::Nav(1)));
        assert_eq!(tip.map(|t| t.line), (mode == 1).then(|| Page::ALL[1].title().to_owned()), "{width}: only icons explain themselves");
    }
    assert_eq!(seen, [true; 3], "every width class is covered");
}

#[test]
fn tab_reaches_the_categories_and_enter_jumps() {
    let mut a = wide();
    a.animations = false;
    a.open_settings(None);
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert_eq!(ui(&a).zone, Zone::List);
    key(&mut a, Key::Tab, NO);
    assert_eq!(ui(&a).zone, Zone::Search);
    key(&mut a, Key::Tab, NO);
    assert_eq!((ui(&a).zone, ui(&a).nav_at), (Zone::Nav, 0));
    let before = a.setting_values();
    key(&mut a, Key::Down, NO);
    key(&mut a, Key::Down, NO);
    key(&mut a, Key::Down, NO);
    key(&mut a, Key::Left, NO);
    key(&mut a, Key::Delete, NO);
    assert_eq!(a.setting_values(), before, "keys in the categories change no setting");
    assert_eq!(ui(&a).nav_at, 3);
    key(&mut a, Key::Enter, NO);
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert_eq!(a.settings_page(), Some(Page::Files));
    assert_eq!(ui(&a).scroll.pos, offset(&a, Page::Files));
    assert_eq!(ui(&a).zone, Zone::Nav, "Enter keeps the categories focused for the next jump");
    key(&mut a, Key::End, NO);
    assert_eq!(ui(&a).nav_at, Page::ALL.len() - 1);
    key(&mut a, Key::Home, NO);
    key(&mut a, Key::Up, NO);
    assert_eq!(ui(&a).nav_at, 0);
    key(&mut a, Key::Down, NO);
    key(&mut a, Key::Down, NO);
    key(&mut a, Key::Tab, NO);
    assert_eq!(ui(&a).zone, Zone::List);
    assert!(matches!(ui(&a).lines[ui(&a).focus], Line::Setting(i) if SETTINGS[i].page == Page::Files), "Tab lands in the section it jumped to");
    key(&mut a, Key::Tab, SHIFT);
    assert_eq!((ui(&a).zone, ui(&a).nav_at), (Zone::Nav, 3), "Shift+Tab goes back to the lit category");
    typed(&mut a, "dec");
    assert_eq!((ui(&a).zone, ui(&a).query()), (Zone::Search, "dec"), "typing in the categories searches");
    let t = a.accessibility();
    assert_eq!(t.by_name(S::SettingsPageLineCategories.t()).unwrap().role, crate::a11y::Role::List);
}

#[test]
fn section_titles_stick_to_the_top_and_keyboard_rows_stay_below_them() {
    let mut a = wide();
    a.open_settings(None);
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let files = offset(&a, Page::Files);
    scroll_to(&mut a, files);
    assert_eq!(ui(&a).sticky, None, "a title already at the top needs no copy");
    scroll_to(&mut a, files + 60.0);
    let (k, r) = ui(&a).sticky.expect("the Files title sticks");
    assert_eq!((k, r.y), (start_of(&a, Page::Files).line, ui(&a).list.y));
    assert_eq!(a.settings_hit(r.x + r.w / 2.0, r.y + r.h / 2.0), Some(Hit::Settings(SHit::Panel)), "rows under the title take no clicks");
    let next = offset(&a, Page::Data);
    scroll_to(&mut a, next - r.h / 2.0);
    let (_, pushed) = ui(&a).sticky.unwrap();
    assert!(pushed.y < ui(&a).list.y && pushed.bottom() <= rect(&a, start_of(&a, Page::Data).line).y + 0.5, "the next title pushes it up");
    scroll_to(&mut a, 0.0);
    let first = ui(&a).first_focusable();
    ui_focus(&mut a, first);
    for k in 0..80 {
        key(&mut a, if k < 40 { Key::Down } else { Key::Up }, NO);
        let u = a.settings.as_mut().unwrap();
        u.scroll.snap(u.scroll.target);
        a.layout();
        if let Some((_, top)) = ui(&a).sticky {
            assert!(rect(&a, ui(&a).focus).y >= top.bottom() - 0.5, "the focused row is not under the title");
        }
    }
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
}

const ALT: Mods = Mods { ctrl: false, shift: false, alt: true };

fn menu_app() -> App {
    let mut a = wide();
    a.animations = false;
    let seen = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    a.opts.menu.seen[0] = seen(&["Open", "Rename", "Send to", "Send to \u{203A} Desktop", "Properties"]);
    a.opts.menu.seen[1] = seen(&["Open in new tab"]);
    a.open_settings(Some(Page::Menu));
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    a
}

fn menu_lines(a: &App) -> Vec<String> {
    a.settings_lines().into_iter().filter(|l| l.starts_with("menu ")).collect()
}

fn menu_row(a: &mut App, label: &str) -> usize {
    let k = ui(a).lines.iter().position(|l| matches!(l, Line::MenuItem(r) if r.label() == label)).unwrap_or_else(|| panic!("{label} is not listed"));
    reveal(a, k)
}

#[test]
fn the_context_menu_section_lists_every_kind_and_holds_its_options() {
    let mut a = app();
    a.open_settings(Some(Page::Menu));
    let lines = page(&a, Page::Menu);
    assert_eq!(lines[..2], [settings::MENU_KIND.to_owned(), S::MenuEditorEmpty.t().to_owned()], "{lines:?}");
    for id in ["general.shell_submenu", "general.menu_open_tab", "advanced.flatten_in_menu"] {
        assert!(lines.contains(&id.to_owned()), "{id} moved into the section");
    }
    assert!(lines.contains(&S::MenuEditorOwn.t().to_owned()) && lines.iter().any(|l| l.contains(S::UserMenuNewEntry.t())));
    assert!(!lines.iter().any(|l| l.contains("menu.pins") || l.contains("menu.seen")), "raw lists stay out of the window");
    assert!(!page(&a, Page::Interface).iter().any(|l| l.starts_with("general.menu")));
    let mut a = menu_app();
    assert_eq!(menu_lines(&a), ["menu [x] Open", "menu [x] Rename", "menu [x] Send to >", "menu [x] Properties"]);
    let k = line(&mut a, settings::MENU_KIND);
    key(&mut a, Key::Right, NO);
    assert_eq!(ui(&a).focus, k);
    assert_eq!(menu_lines(&a), ["menu [x] Open in new tab"], "the folder menu has its own list");
    key(&mut a, Key::Right, NO);
    assert!(menu_lines(&a).is_empty());
    assert!(a.settings_lines().contains(&S::MenuEditorEmpty.t().to_owned()));
}

#[test]
fn space_hides_enter_pins_or_folds_and_the_mouse_does_the_same() {
    let mut a = menu_app();
    menu_row(&mut a, "Rename");
    key(&mut a, Key::Space, NO);
    assert_eq!(a.opts.menu.hidden[0], ["Rename"]);
    assert!(menu_lines(&a).contains(&"menu [ ] Rename".to_owned()));
    let k = menu_row(&mut a, "Rename");
    let target = center(menu_editor::check_rect(rect(&a, k), 0, s));
    click_at(&mut a, target);
    assert!(a.opts.menu.hidden[0].is_empty(), "a click on the box shows it again");
    menu_row(&mut a, "Open");
    key(&mut a, Key::Enter, NO);
    assert_eq!(a.opts.pins[0], ["Open"]);
    let k = menu_row(&mut a, "Properties");
    let target = center(menu_editor::star_rect(rect(&a, k), s));
    click_at(&mut a, target);
    assert_eq!(a.opts.pins[0], ["Properties", "Open"]);
    assert!(menu_lines(&a).contains(&"menu [x] Properties *".to_owned()));
    menu_row(&mut a, "Send to");
    key(&mut a, Key::Enter, NO);
    assert!(menu_lines(&a).contains(&"menu   [x] Desktop".to_owned()), "{:?}", menu_lines(&a));
    let k = menu_row(&mut a, "Desktop");
    key(&mut a, Key::Space, NO);
    assert_eq!(a.opts.menu.hidden[0], ["Send to \u{203A} Desktop"]);
    let target = center(menu_editor::star_rect(rect(&a, k), s));
    click_at(&mut a, target);
    assert_eq!(a.opts.pins[0][0], "Send to \u{203A} Desktop", "nested commands pin by their path");
    let k = menu_row(&mut a, "Send to");
    let target = center(menu_editor::fold_rect(rect(&a, k), 0, s));
    click_at(&mut a, target);
    assert!(!menu_lines(&a).iter().any(|l| l.contains("Desktop")), "the chevron folds it again");
}

#[test]
fn rows_move_with_alt_arrows_and_by_dragging() {
    let mut a = menu_app();
    menu_row(&mut a, "Properties");
    key(&mut a, Key::Up, ALT);
    assert_eq!(a.opts.menu.order[0], ["Open", "Rename", "Properties", "Send to"]);
    assert!(matches!(&ui(&a).lines[ui(&a).focus], Line::MenuItem(r) if r.label() == "Properties"), "the moved row keeps the focus");
    key(&mut a, Key::Up, ALT);
    key(&mut a, Key::Up, ALT);
    key(&mut a, Key::Up, ALT);
    assert_eq!(a.opts.menu.order[0], ["Properties", "Open", "Rename", "Send to"], "the top stops the row");
    key(&mut a, Key::Down, ALT);
    assert_eq!(a.opts.menu.order[0], ["Open", "Properties", "Rename", "Send to"]);
    let at = menu_row(&mut a, "Send to");
    let from = rect(&a, at);
    let at = menu_row(&mut a, "Open");
    let onto = rect(&a, at);
    let (x, y) = (from.x + from.w / 2.0, from.y + from.h / 2.0);
    a.on_event(&Event::MouseMove { x, y }, Instant::now());
    a.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseMove { x, y: onto.y + 3.0 }, Instant::now());
    let dragged = menu_row(&mut a, "Send to");
    assert_eq!(a.drag, super::super::Drag::MenuMove { from: dragged, to: Some(0) });
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    a.on_event(&Event::MouseUp { x, y: onto.y + 3.0, button: MouseButton::Left }, Instant::now());
    assert_eq!(a.opts.menu.order[0], ["Send to", "Open", "Properties", "Rename"]);
    let at = menu_row(&mut a, "Rename");
    let r = rect(&a, at);
    click_at(&mut a, center(r));
    assert_eq!(a.opts.menu.order[0], ["Send to", "Open", "Properties", "Rename"], "a click without a drag moves nothing");
}

#[test]
fn reset_restores_one_menu_and_leaves_the_others() {
    let mut a = menu_app();
    a.opts.menu.hidden = [vec!["Rename".into()], vec!["Open in new tab".into()], Vec::new()];
    a.opts.menu.order[0] = vec!["Properties".into()];
    a.opts.pins[0] = vec!["Open".into()];
    a.layout();
    let k = ui(&a).lines.iter().position(|l| *l == Line::Buttons(None, &[Btn::MenuReset])).expect("the section has a reset button");
    reveal(&mut a, k);
    key(&mut a, Key::Enter, NO);
    assert!(a.opts.menu.hidden[0].is_empty() && a.opts.menu.order[0].is_empty());
    assert_eq!(a.opts.menu.hidden[1], ["Open in new tab"], "the folder menu keeps its layout");
    assert_eq!(a.opts.pins[0], ["Open"], "pins stay: they have their own star");
    assert_eq!(menu_lines(&a), ["menu [x] Open *", "menu [x] Rename", "menu [x] Send to >", "menu [x] Properties"]);
}

#[test]
fn the_editor_has_accessible_rows_and_draws_in_both_themes() {
    let mut a = menu_app();
    let k = menu_row(&mut a, "Open");
    let mut t = crate::a11y::Tree::new("Flit", Rect::new(0.0, 0.0, 1400.0, 900.0));
    let mut targets = Vec::new();
    a.settings_a11y(&mut t, &mut targets);
    let row = t.nodes.iter().find(|n| n.name == "Open" && n.role == Role::CheckBox).expect("a row is a checkbox");
    assert!(row.selected);
    assert!(t.nodes.iter().any(|n| n.name == S::MenuEditorPin.t() && n.role == Role::Button));
    assert!(targets.iter().any(|(_, h)| *h == Hit::Settings(SHit::Check(k))));
    for dark in [false, true] {
        a.on_event(&Event::ThemeChanged { dark }, Instant::now());
        let mut g = Gfx::new(Mono, 1024);
        a.frame(&mut g, Instant::now());
        assert!(g.dl.instances.len() > 50);
    }
}
