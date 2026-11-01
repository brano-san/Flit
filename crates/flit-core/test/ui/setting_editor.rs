use super::*;
use crate::input::{Event, Key, Mods};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::settings::index;
use crate::ui::{Page, Persisted};
use std::time::Instant;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };

fn app() -> App {
    let mut a = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1100, 720), Instant::now());
    a.drain_effects().for_each(drop);
    a.open_settings(Some(Page::Colors));
    a.drain_effects().for_each(drop);
    a
}

fn key(a: &mut App, k: Key) {
    a.on_event(&Event::Key { key: k, mods: NO, pressed: true }, Instant::now());
}

fn typed(a: &mut App, text: &str) {
    a.on_event(&Event::Key { key: Key::Char('A'), mods: Mods { ctrl: true, shift: false, alt: false }, pressed: true }, Instant::now());
    for c in text.chars() {
        a.on_event(&Event::Char(c), Instant::now());
    }
}

fn pick(a: &App) -> Option<SettingPick> {
    match a.nav.palette.as_ref().map(|p| &p.pick) {
        Some(Pick::Setting(p)) => Some(p.clone()),
        _ => None,
    }
}

fn notes(a: &App) -> Vec<Note> {
    let p = a.nav.palette.as_ref().unwrap();
    let Pick::Setting(sp) = &p.pick else { return Vec::new() };
    let text = p.chosen(p.sel).unwrap_or(p.field.text()).to_owned();
    a.setting_notes(sp, &text)
}

fn verdict(a: &App) -> Tone {
    notes(a).last().unwrap().tone
}

fn get(a: &App, id: &str) -> Value {
    SETTINGS[index(id).unwrap()].get(a)
}

fn titles(a: &App) -> Vec<String> {
    let p = a.nav.palette.as_ref().unwrap();
    p.matches.iter().map(|&k| p.titles[k].clone()).collect()
}

#[test]
fn the_color_editor_explains_previews_and_refuses_a_bad_color() {
    let mut a = app();
    let i = index("appearance.background").unwrap();
    a.activate_setting(i, None);
    assert_eq!(pick(&a), Some(SettingPick::Color(i)));
    let n = notes(&a);
    assert_eq!(n.len(), 3);
    assert!(n[0].text.contains("#RRGGBB") || n[0].text.contains(EXAMPLE_COLOR), "{}", n[0].text);
    assert!(n[1].text.starts_with("Current: "), "{}", n[1].text);
    assert_eq!(titles(&a).len(), PRESETS.len() + 1);
    assert_eq!(a.nav.palette.as_ref().unwrap().sel, usize::MAX, "no suggestion is picked until the arrows move");
    typed(&mut a, "#12");
    assert_eq!(verdict(&a), Tone::Bad);
    key(&mut a, Key::Enter);
    assert_eq!(pick(&a), Some(SettingPick::Color(i)), "a bad color keeps the editor open");
    assert_eq!(get(&a, "appearance.background"), Value::Text(String::new()));
    typed(&mut a, "#F5F0E1");
    assert_eq!(verdict(&a), Tone::Good);
    assert_eq!(notes(&a).last().unwrap().swatch, Some(Color::rgb(0xF5F0E1)), "the preview shows the typed color");
    key(&mut a, Key::Enter);
    assert_eq!(pick(&a), None);
    assert_eq!(get(&a, "appearance.background"), Value::Text("#F5F0E1".into()));
    a.activate_setting(i, None);
    key(&mut a, Key::Down);
    key(&mut a, Key::Down);
    assert!(notes(&a).last().unwrap().swatch.is_some(), "the preview follows the highlighted preset");
    key(&mut a, Key::Enter);
    assert_eq!(get(&a, "appearance.background"), Value::Text(PRESETS[0].id.into()));
}

#[test]
fn the_background_image_editor_checks_the_path_and_type_and_browses() {
    let mut a = app();
    let i = index(BACKGROUND_IMAGE).unwrap();
    a.activate_setting(i, None);
    assert_eq!(pick(&a), Some(SettingPick::Text(i)));
    let rows = titles(&a);
    assert!(rows.contains(&S::SettingsPageGuideNoImage.t().to_owned()));
    assert_eq!(rows.contains(&S::SettingsPageRowBrowse.t().to_owned()), cfg!(windows), "{rows:?}");
    typed(&mut a, "wallpaper");
    assert_eq!(verdict(&a), Tone::Bad, "a bare name is not a full path");
    typed(&mut a, r"C:\x\notes.txt");
    assert_eq!(verdict(&a), Tone::Bad);
    assert_eq!(notes(&a).last().unwrap().text, S::SettingsPageGuideImageType.t());
    typed(&mut a, r"C:\x\wall.PNG");
    assert_eq!(verdict(&a), Tone::Good);
    key(&mut a, Key::Enter);
    assert_eq!(a.look.backdrop_path, r"C:\x\wall.PNG");
    let gen = a.drain_effects().find_map(|e| if let Effect::Backdrop { gen, .. } = e { Some(gen) } else { None }).expect("the image loads");
    a.on_backdrop(gen, None);
    assert!(a.text_value(i).contains("could not be loaded"), "{}", a.text_value(i));
    a.setting_input(SettingPick::Text(i), BROWSE);
    let browse = a.drain_effects().find_map(|e| if let Effect::Settings(SettingsCmd::Browse { exts, folder, start }) = e { Some((exts, folder, start)) } else { None });
    let (exts, folder, start) = browse.expect("the browse row asks the platform for a file");
    assert!(exts.contains(&"jpg") && !folder);
    assert_eq!(start, FsPath::parse(r"C:\x"), "the dialog starts where the current image is");
    a.on_browsed(FsPath::parse(r"C:\x\sea.jpg"));
    assert_eq!(a.look.backdrop_path, r"C:\x\sea.jpg");
    a.on_browsed(FsPath::parse(r"C:\x\other.jpg"));
    assert_eq!(a.look.backdrop_path, r"C:\x\sea.jpg", "only an asked dialog answers");
    a.setting_input(SettingPick::Text(i), "");
    assert_eq!(a.text_value(i), S::SettingsPageGuideEmpty.t());
}

#[test]
fn the_inspector_font_editor_waits_for_fonts_and_lists_monospaced_ones_first() {
    let mut a = app();
    let i = index(INSPECTOR_FONT).unwrap();
    a.activate_setting(i, None);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Settings(SettingsCmd::Fonts))));
    assert_eq!(pick(&a), None);
    a.set_fonts(vec!["Arial".into(), "Cascadia Mono".into(), "Consolas".into()]);
    assert_eq!(pick(&a), Some(SettingPick::Text(i)));
    assert_eq!(titles(&a), ["Cascadia Mono", "Consolas", "Arial"]);
    typed(&mut a, "Nope Mono");
    assert_eq!(verdict(&a), Tone::Warn);
    typed(&mut a, "consolas");
    assert_eq!(verdict(&a), Tone::Good);
    typed(&mut a, &"x".repeat(80));
    assert_eq!(verdict(&a), Tone::Bad);
    typed(&mut a, "Nope Mono");
    key(&mut a, Key::Enter);
    assert_eq!(a.opts.inspector_font, "Nope Mono", "a missing font is a warning, not a refusal");
}

#[test]
fn the_folder_editor_suggests_places_and_refuses_listed_ones() {
    let mut a = app();
    let i = index("general.startup_pages").unwrap();
    a.open_setting_editor(SettingPick::AddPath(i), "");
    let rows = titles(&a);
    assert!(rows.contains(&S::SettingsPageRowHome.t().to_owned()) && rows.contains(&S::SettingsPageRowCurrentFolder.t().to_owned()), "{rows:?}");
    let here = rows.iter().position(|r| r == S::SettingsPageRowCurrentFolder.t()).unwrap();
    a.run_palette_item(here, Instant::now());
    assert_eq!(get(&a, "general.startup_pages"), Value::Paths(vec![Location::parse(r"C:\x").unwrap()]));
    a.open_setting_editor(SettingPick::AddPath(i), r"C:\x");
    assert!(!titles(&a).contains(&S::SettingsPageRowCurrentFolder.t().to_owned()), "a listed folder is not suggested again");
    assert_eq!(verdict(&a), Tone::Bad);
    key(&mut a, Key::Enter);
    assert!(pick(&a).is_some(), "Enter on a listed folder keeps the editor open");
}

#[test]
fn export_and_import_editors_warn_about_a_name_that_is_not_json() {
    let mut a = app();
    a.ask_export_settings();
    assert_eq!(pick(&a), Some(SettingPick::Export));
    assert_eq!(verdict(&a), Tone::Good);
    typed(&mut a, r"C:\x\settings.txt");
    assert_eq!(verdict(&a), Tone::Warn);
    typed(&mut a, "settings");
    assert_eq!(verdict(&a), Tone::Bad);
    a.ask_import_settings();
    assert_eq!(pick(&a), Some(SettingPick::Import));
    assert!(titles(&a).contains(&S::SettingsPageRowSuggested.t().to_owned()));
}

#[test]
fn an_editor_draws_its_notes_and_swatches_in_both_themes() {
    for dark in [false, true] {
        let mut a = app();
        a.on_event(&Event::ThemeChanged { dark }, Instant::now());
        a.activate_setting(index("appearance.color_files").unwrap(), None);
        let without = {
            let mut g = Gfx::new(Mono, 1024);
            a.frame(&mut g, Instant::now());
            g.dl.instances.len()
        };
        assert!(without > 0);
        let field = a.palette_field().unwrap();
        let first = a.nav.palette.as_ref().unwrap().rows[0];
        assert!(first.y >= field.bottom() + NOTE_LINES as f32 * a.note_h(), "the notes sit between the field and the suggestions");
    }
}
