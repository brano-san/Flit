use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::location::Location;
use crate::render::Color;
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::theme::{DARK, LIGHT};
use crate::ui::settings::index;
use crate::ui::{MenuItem, Page, Persisted};
use std::time::Instant;

const CTRL_ALT: Mods = Mods { ctrl: true, shift: false, alt: true };

fn app() -> App {
    let mut a = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1100, 720), Instant::now());
    a.drain_effects().for_each(drop);
    a
}

fn set(a: &mut App, id: &str, v: Value) -> bool {
    SETTINGS[index(id).unwrap()].set(a, v)
}

fn get(a: &App, id: &str) -> Value {
    SETTINGS[index(id).unwrap()].get(a)
}

fn text(s: &str) -> Value {
    Value::Text(s.to_owned())
}

fn frame(a: &mut App) -> Gfx<Mono> {
    let mut g = Gfx::new(Mono, 1024);
    a.frame(&mut g, Instant::now());
    g
}

fn has_rect(g: &Gfx<Mono>, c: Color) -> bool {
    g.dl.instances.iter().any(|i| i.kind == crate::render::kind::SOLID && i.color == c.0)
}

fn menu_after(a: &mut App, f: impl FnOnce(&mut App)) -> Vec<MenuItem> {
    a.drain_effects().for_each(drop);
    f(a);
    a.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).expect("a menu")
}

#[test]
fn the_theme_follows_windows_or_the_setting_and_ctrl_alt_t_switches_it() {
    let mut a = app();
    assert!(!a.is_dark() && *a.theme() == LIGHT);
    a.on_event(&Event::ThemeChanged { dark: true }, Instant::now());
    assert!(a.is_dark() && *a.theme() == DARK, "the system setting by default");
    assert!(set(&mut a, "appearance.theme", Value::Choice(1)));
    assert!(!a.is_dark());
    a.on_event(&Event::ThemeChanged { dark: false }, Instant::now());
    a.on_event(&Event::ThemeChanged { dark: true }, Instant::now());
    assert!(!a.is_dark(), "a chosen theme ignores Windows");
    a.on_event(&Event::Key { key: Key::Char('T'), mods: CTRL_ALT, pressed: true }, Instant::now());
    assert!(a.is_dark());
    assert_eq!(get(&a, "appearance.theme"), Value::Choice(2));
    a.on_event(&Event::Key { key: Key::Char('T'), mods: CTRL_ALT, pressed: true }, Instant::now());
    assert_eq!(get(&a, "appearance.theme"), Value::Choice(1));
    for (id, dark) in [("theme.dark", true), ("theme.light", false), ("theme.system", true)] {
        let action = crate::ui::actions::ACTIONS.iter().find(|x| x.id == id).unwrap();
        assert!(a.available(id));
        (action.run.unwrap())(&mut a, Instant::now());
        assert_eq!(a.is_dark(), dark, "{id}");
    }
    let clash: Vec<_> = crate::ui::actions::ACTIONS.iter().filter(|x| x.keys.iter().any(|c| c.key == Key::Char('T') && c.mods == CTRL_ALT)).map(|x| x.id).collect();
    assert_eq!(clash, ["theme.toggle"]);
}

#[test]
fn colors_are_picked_from_presets_or_typed_and_checked() {
    let mut a = app();
    a.open_settings(Some(Page::Appearance));
    let i = index("appearance.background").unwrap();
    let menu = menu_after(&mut a, |a| a.activate_setting(i, None));
    assert_eq!(menu.len(), PRESETS.len() + 2);
    assert!(matches!(&menu[0], MenuItem::Command { checked: true, .. }));
    a.on_event(&Event::MenuCommand(11), Instant::now());
    assert_eq!(get(&a, "appearance.background"), text("storm"));
    assert_eq!(a.theme().bg, Color::rgb(0xD9DFE8));
    assert_eq!(a.pick_text(i).as_deref(), Some("Storm"));
    assert_eq!(a.swatch(i), Some(Color::rgb(0xD9DFE8)));
    a.on_event(&Event::ThemeChanged { dark: true }, Instant::now());
    assert_eq!(a.theme().bg, Color::rgb(0x1D2533), "the preset has a dark shade");
    a.activate_setting(i, None);
    a.on_event(&Event::MenuCommand(PRESETS.len() + 1), Instant::now());
    assert!(matches!(a.nav.palette.as_ref().map(|p| &p.pick), Some(Pick::Setting(SettingPick::Color(_)))));
    a.setting_input(SettingPick::Color(i), "#F5F0E1");
    assert_eq!(get(&a, "appearance.background"), text("#F5F0E1"));
    assert!(!a.is_dark(), "a light custom color makes the text dark even in the dark theme");
    a.setting_input(SettingPick::Color(i), "#12345");
    assert_eq!(get(&a, "appearance.background"), text("#F5F0E1"));
    assert!(a.tab().error.as_deref().is_some_and(|e| e.contains("six hexadecimal digits")));
    assert!(!set(&mut a, "appearance.color_files", text("teal")));
    assert!(SETTINGS[i].from_json(&serde_json::json!("nonsense")).is_none());
    assert_eq!(SETTINGS[i].from_json(&serde_json::json!("Gold")), Some(text("Gold")));
}

#[test]
fn area_colors_paint_their_area() {
    let mut a = app();
    for (id, rgb) in [("appearance.color_toolbar", 0x203040), ("appearance.color_sidebar", 0x405060), ("appearance.color_files", 0xFFF8E0), ("appearance.color_pane", 0x600000), ("appearance.color_address", 0xE0FFE0)] {
        assert!(set(&mut a, id, text(&format!("#{rgb:06X}"))), "{id}");
    }
    a.toggle_preview_pane();
    let g = frame(&mut a);
    assert_eq!(g.dl.clear, Color::rgb(0xFFF8E0), "the file area");
    for rgb in [0x203040, 0x405060, 0x600000, 0xE0FFE0] {
        assert!(has_rect(&g, Color::rgb(rgb)), "{rgb:06X}");
    }
    assert!(crate::theme::contrast(a.look.palette.area(Area::Toolbar).text, Color::rgb(0x203040)) >= crate::theme::MIN_TEXT_CONTRAST);
    assert!(has_rect(&g, LIGHT.chrome), "the tab strip keeps the main colors");
    assert!(set(&mut a, "appearance.color_toolbar", text("")));
    assert_eq!(*a.look.palette.area(Area::Toolbar), LIGHT);
}

#[test]
fn the_font_comes_from_the_installed_families_and_falls_back() {
    let mut a = app();
    a.open_settings(Some(Page::Appearance));
    let i = index("appearance.font").unwrap();
    a.activate_setting(i, None);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Settings(SettingsCmd::Fonts))), "the families are listed only when asked");
    assert!(a.nav.palette.is_none());
    a.set_fonts(vec!["Arial".into(), "Bahnschrift".into(), "Consolas".into()]);
    let p = a.nav.palette.as_ref().expect("the font list opens");
    assert_eq!(p.titles, [S::AppearanceSystemFont.t(), "Arial", "Bahnschrift", "Consolas"]);
    a.palette_insert("bahn");
    let sel = a.nav.palette.as_ref().unwrap().sel;
    a.run_palette_item(sel, Instant::now());
    assert_eq!(get(&a, "appearance.font"), text("Bahnschrift"));
    frame(&mut a);
    assert!(a.look.font_missing, "the test backend has no fonts, so it falls back");
    assert_eq!(a.pick_text(i).as_deref(), Some("Bahnschrift (not installed)"));
    a.activate_setting(i, None);
    assert!(a.drain_effects().all(|e| !matches!(e, Effect::Settings(SettingsCmd::Fonts))), "the list is kept");
    a.run_palette_item(0, Instant::now());
    assert_eq!(get(&a, "appearance.font"), text(""));
    frame(&mut a);
    assert_eq!(a.pick_text(i).as_deref(), Some(S::AppearanceSystemFont.t()));
    assert!(!set(&mut a, "appearance.font", text("Bad\u{7}Name")));
}

#[test]
fn bars_and_buttons_follow_their_settings() {
    let mut a = app();
    assert!(a.layout.status.h > 0.0 && a.layout.actions.w > 0.0 && a.layout.ops_button.w == 0.0);
    assert!(set(&mut a, "appearance.status_bar", Value::Bool(false)));
    assert!(set(&mut a, "appearance.tab_actions", Value::Bool(false)));
    assert!(set(&mut a, "appearance.ops_button", Value::Choice(1)));
    a.layout();
    assert!(a.layout.status.h == 0.0 && a.layout.actions.w == 0.0 && a.layout.ops_button.w > 0.0);
    a.status("Something to say");
    a.layout();
    assert!(a.layout.status.h > 0.0, "a message still shows the status bar");
    assert!(set(&mut a, "appearance.item_count", Value::Choice(1)));
    assert!(a.prefs.split_count);
}

#[test]
fn the_appearance_page_draws_in_both_themes_with_custom_colors() {
    let mut a = app();
    assert!(set(&mut a, "appearance.background", text("camouflage")));
    for dark in [false, true] {
        a.on_event(&Event::ThemeChanged { dark }, Instant::now());
        a.open_settings(Some(Page::Appearance));
        let g = frame(&mut a);
        assert!(g.dl.instances.len() > 50);
        a.on_event(&Event::MouseMove { x: 500.0, y: 300.0 }, Instant::now());
        a.on_event(&Event::MouseUp { x: 500.0, y: 300.0, button: MouseButton::Left }, Instant::now());
    }
}

#[test]
fn high_contrast_overrides_the_theme_and_follows_live_changes() {
    let mut a = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    let colors = SystemColors {
        window: Color::rgb(0xFFFFFF),
        window_text: Color::rgb(0x000000),
        highlight: Color::rgb(0x37006E),
        highlight_text: Color::rgb(0xFFFFFF),
        hotlight: Color::rgb(0x00009F),
        gray_text: Color::rgb(0x600000),
        btn_face: Color::rgb(0xFFFFFF),
    };
    a.on_event(&Event::Contrast(Some(colors)), Instant::now());
    assert_eq!(a.look.palette.base, crate::theme::high_contrast(&colors));
    a.on_event(&Event::Contrast(None), Instant::now());
    assert_eq!(a.look.palette.base, LIGHT);
}

#[test]
fn cover_crops_the_longer_side_of_the_image_around_its_center() {
    assert_eq!(cover_uv(1.0, 2.0), [0.25, 0.0, 0.75, 1.0]);
    assert_eq!(cover_uv(2.0, 1.0), [0.0, 0.25, 1.0, 0.75]);
    assert_eq!(cover_uv(1.5, 1.5), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn a_background_image_loads_in_the_background_and_draws_dimmed_behind_everything() {
    use crate::render::kind::BACKDROP;
    let mut a = app();
    assert!(set(&mut a, "appearance.background_image", text(r" C:\pics\sea.jpg ")));
    assert_eq!(get(&a, "appearance.background_image"), text(r"C:\pics\sea.jpg"));
    let gen = match a.drain_effects().find(|e| matches!(e, Effect::Backdrop { .. })) {
        Some(Effect::Backdrop { gen, path }) => {
            assert_eq!(path, FsPath::parse(r"C:\pics\sea.jpg").unwrap());
            gen
        }
        _ => panic!("no backdrop request"),
    };
    let (rev, image) = a.backdrop_image();
    assert!(image.is_none());
    a.on_backdrop(gen + 1, Some(Bitmap { width: 1, height: 1, rgba: vec![9; 4] }));
    assert_eq!(a.backdrop_image().0, rev);
    assert!(!frame(&mut a).dl.instances.iter().any(|i| i.kind == BACKDROP));
    a.on_backdrop(gen, Some(Bitmap { width: 4, height: 2, rgba: vec![9; 32] }));
    assert_ne!(a.backdrop_image().0, rev);
    let g = frame(&mut a);
    assert_eq!(g.dl.instances.iter().position(|i| i.kind == BACKDROP), Some(0));
    assert!(g.dl.instances[0].color >> 24 < 0x80);
    a.run_action("view.clear_backdrop", Instant::now());
    assert_eq!(get(&a, "appearance.background_image"), text(""));
    assert!(a.backdrop_image().1.is_none());
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::Backdrop { .. })));
    assert!(!frame(&mut a).dl.instances.iter().any(|i| i.kind == BACKDROP));
}

#[test]
fn a_failed_background_image_says_so_in_the_status_bar() {
    let mut a = app();
    set(&mut a, "appearance.background_image", text(r"C:\pics\broken.jpg"));
    let Some(Effect::Backdrop { gen, .. }) = a.drain_effects().find(|e| matches!(e, Effect::Backdrop { .. })) else { panic!() };
    a.on_backdrop(gen, None);
    assert!(a.tab().error.as_deref().is_some_and(|e| e.contains("broken.jpg")));
}