use super::*;
use crate::tags::COLORS;
use crate::open::Route;
use crate::ui::Tab;
use std::collections::HashSet;

fn app() -> App {
    App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now())
}

fn get(a: &App, id: &str) -> Value {
    SETTINGS[index(id).unwrap()].get(a)
}

fn set(a: &mut App, id: &str, v: Value) -> bool {
    SETTINGS[index(id).unwrap()].set(a, v)
}

#[test]
fn every_setting_has_a_unique_id_a_title_a_description_and_a_page() {
    let mut seen = HashSet::new();
    for s in SETTINGS {
        assert!(seen.insert(s.id), "duplicate id {}", s.id);
        let prefix = match s.page {
            Page::General => "general",
            Page::Appearance => "appearance",
            Page::Files => "files",
            Page::Layout => "layout",
            Page::Advanced => "advanced",
            Page::Actions => "keys",
            Page::Tags | Page::About => "",
        };
        assert_eq!(s.id.split('.').next(), Some(prefix), "the id of {} starts with its page", s.id);
        assert!(!s.title.t().is_empty() && !s.help.t().is_empty(), "{}", s.id);
        assert!(Page::ALL.contains(&s.page));
        if let Kind::Choice(c, ..) = s.kind {
            let keys: HashSet<_> = c.iter().map(|c| c.0).collect();
            assert_eq!(keys.len(), c.len(), "{} has duplicate choice keys", s.id);
        }
        if let Kind::Number { min, max, step, .. } = s.kind {
            assert!(min < max && step > 0, "{}", s.id);
        }
    }
    assert!(SETTINGS.len() >= 50, "the registry covers the settings pages");
}

#[test]
fn defaults_are_the_values_of_a_fresh_app_and_survive_json() {
    let fresh = app();
    assert_eq!(defaults(), fresh.setting_values().as_slice());
    for (s, v) in SETTINGS.iter().zip(defaults()) {
        assert_eq!(s.from_json(&s.to_json(v)).as_ref(), Some(v), "{}", s.id);
    }
    let text = render(defaults(), &Map::new(), None);
    let doc = parse(&text);
    assert!(doc.problems.is_empty() && doc.bad.is_empty() && !doc.broken);
    assert_eq!(doc.values.len(), SETTINGS.len());
    let mut values = doc.values.clone();
    values.sort_by_key(|(i, _)| *i);
    assert_eq!(values.into_iter().map(|(_, v)| v).collect::<Vec<_>>(), defaults());
}

#[test]
fn system_integration_is_off_by_default_and_follows_the_file() {
    assert_eq!(defaults()[index(DEFAULT_MANAGER).unwrap()], Value::Bool(false));
    let mut a = app();
    let doc = parse(r#"{"advanced.default_manager": true, "advanced.open_in_flit": true}"#);
    a.apply_doc(&doc);
    assert!(a.default_manager);
    assert!(doc.unknown.contains_key("advanced.open_in_flit"), "the removed setting is kept as an unknown key");
    assert_eq!(get(&a, DEFAULT_MANAGER), Value::Bool(true));
    assert!(a.export_text().contains("\"advanced.default_manager\": true"));
    set(&mut a, DEFAULT_MANAGER, Value::Bool(false));
    assert!(!a.default_manager);
}

#[test]
fn every_toggle_and_choice_reads_back_what_was_set() {
    let mut a = app();
    for s in SETTINGS {
        match s.kind {
            Kind::Toggle(..) => {
                let Value::Bool(b) = s.get(&a) else { panic!() };
                assert!(s.set(&mut a, Value::Bool(!b)));
                assert_eq!(s.get(&a), Value::Bool(!b), "{}", s.id);
            }
            Kind::Choice(c, ..) => {
                for k in (0..c.len()).rev() {
                    assert!(s.set(&mut a, Value::Choice(k)));
                    assert_eq!(s.get(&a), Value::Choice(k), "{}", s.id);
                }
                assert!(!s.set(&mut a, Value::Choice(c.len())), "{} accepts an index past its choices", s.id);
            }
            Kind::Number { min, max, .. } => {
                assert!(s.set(&mut a, Value::Number(max)));
                assert_eq!(s.get(&a), Value::Number(max));
                assert!(!s.set(&mut a, Value::Number(min - 1)));
                assert!(!s.set(&mut a, Value::Number(max + 1)));
            }
            Kind::Paths(..) => {
                let pages = vec![Location::home_page(), Location::parse(r"D:\Projects").unwrap()];
                assert!(s.set(&mut a, Value::Paths(pages.clone())));
                assert_eq!(s.get(&a), Value::Paths(pages));
            }
            Kind::Text(..) | Kind::Pick(..) | Kind::Keys(..) => {}
        }
        assert!(!s.set(&mut a, Value::Text("wrong kind".into())) || matches!(s.kind, Kind::Text(..) | Kind::Pick(..)));
    }
}

#[test]
fn unknown_keys_stay_and_bad_values_fall_back_to_defaults_with_a_note() {
    let text = r##"{
        "files.show_hidden": true,
        "files.size_units": "decimal",
        "layout.default_layout": "zigzag",
        "advanced.thumb_cache_mb": 99999,
        "general.startup_pages": ["Home", "C:\\Work"],
        "theme.accent": "#FF0000",
        "future.option": {"nested": [1, 2]}
    }"##;
    let doc = parse(text);
    assert!(!doc.broken);
    assert_eq!(doc.value("files.show_hidden"), Some(&Value::Bool(true)));
    assert_eq!(doc.value("general.startup_pages"), Some(&Value::Paths(vec![Location::home_page(), Location::parse(r"C:\Work").unwrap()])));
    assert_eq!(doc.bad, [index("advanced.thumb_cache_mb").unwrap(), index("layout.default_layout").unwrap()]);
    assert_eq!(doc.unknown.keys().collect::<Vec<_>>(), ["future.option", "theme.accent"]);
    let mut a = app();
    set(&mut a, "layout.default_layout", Value::Choice(2));
    a.apply_doc(&doc);
    assert!(a.show_hidden && a.style.decimal);
    assert_eq!(get(&a, "layout.default_layout"), defaults()[index("layout.default_layout").unwrap()]);
    let note = a.tab().error.clone().unwrap();
    assert!(note.contains("advanced.thumb_cache_mb") && note.contains("1 more problem"), "{note}");
    let out = render(&a.setting_values(), &doc.unknown, None);
    let again = parse(&out);
    assert_eq!(again.unknown, doc.unknown);
    assert!(again.bad.is_empty());
    assert!(out.contains("\"theme.accent\": \"#FF0000\""));
}

#[test]
fn a_file_that_is_not_json_changes_nothing_and_says_so() {
    for text in ["", "{", "[1, 2]", "\"text\"", "{\"files.show_hidden\": tru}"] {
        let doc = parse(text);
        assert!(doc.broken, "{text:?}");
        let mut a = app();
        let before = a.setting_values();
        a.apply_doc(&doc);
        assert_eq!(a.setting_values(), before);
        assert!(a.tab().error.as_deref().is_some_and(|e| e.contains("not valid JSON")));
    }
    let doc = parse("\u{FEFF}{\"files.show_hidden\": true}");
    assert!(!doc.broken && doc.value("files.show_hidden") == Some(&Value::Bool(true)), "a byte order mark is fine");
}

#[test]
fn export_carries_tags_and_import_creates_and_recolors_them() {
    let mut a = app();
    set(&mut a, "files.confirm_delete", Value::Choice(0));
    let exported = a.export_text();
    assert!(exported.contains("\"files.confirm_delete\": \"always\""));
    assert!(exported.contains("\"tags\": []"));
    let mut b = app();
    let text = format!(
        r##"{{"files.confirm_delete": "always", "general.date_format": "application", "tags": [{{"name": "Work", "color": "#{:06X}"}}, {{"name": ".bad", "color": "#000000"}}, {{"name": "NoColor"}}]}}"##,
        COLORS[2].1
    );
    b.import_settings(&text);
    assert_eq!(b.confirm_delete, ConfirmDelete::Always);
    assert_eq!(b.style.dates, DateFormat::Application);
    let created: Vec<_> = b.drain_effects().filter_map(|e| if let Effect::Tags(TagRequest::Create { name, color, .. }) = e { Some((name, color)) } else { None }).collect();
    assert_eq!(created, [("Work".to_owned(), COLORS[2].1)]);
    assert!(b.tab().error.as_deref().is_some_and(|e| e.starts_with("Imported 2 settings and 1 tag")));
    b.import_settings("not json");
    assert!(b.tab().error.as_deref().is_some_and(|e| e.contains("not valid JSON")));
}

#[test]
fn reset_brings_back_every_default() {
    let mut a = app();
    for (s, v) in SETTINGS.iter().zip(defaults()) {
        if let (Kind::Toggle(..), Value::Bool(b)) = (s.kind, v) {
            s.set(&mut a, Value::Bool(!b));
        }
    }
    assert_ne!(a.setting_values(), defaults());
    a.reset_settings();
    assert_eq!(a.setting_values(), defaults());
    assert_eq!(a.tab().error.as_deref(), Some("All settings are back to their defaults"));
}

#[test]
fn startup_comes_from_the_file_with_its_pages() {
    let doc = parse(r#"{"general.startup": "pages", "general.startup_pages": ["Home", "D:\\x"]}"#);
    assert_eq!(doc.startup(), Some(Startup::Folders(vec![Location::home_page(), Location::parse(r"D:\x").unwrap()])));
    assert_eq!(parse(r#"{"general.startup": "new_tab"}"#).startup(), Some(Startup::NewTab));
    assert_eq!(parse("{}").startup(), None);
    let mut a = app();
    a.apply_doc(&doc);
    assert_eq!(a.startup, Startup::Folders(vec![Location::home_page(), Location::parse(r"D:\x").unwrap()]));
    assert_eq!(a.persisted().startup, a.startup);
}

#[test]
fn text_settings_round_trip_with_a_length_limit() {
    static NOTE: Setting = Setting { id: "test.note", page: Page::Advanced, title: crate::i18n::S::SettingsTitleGeneralLanguage, help: crate::i18n::S::SettingsHelpGeneralLanguage, kind: Kind::Text(|a| a.about.os.clone(), |a, t| a.about.os = t) };
    let mut a = app();
    assert!(NOTE.set(&mut a, Value::Text("hello".into())));
    assert_eq!(NOTE.get(&a), Value::Text("hello".into()));
    assert_eq!(NOTE.to_json(&NOTE.get(&a)), Json::String("hello".into()));
    assert_eq!(NOTE.from_json(&Json::String("x".into())), Some(Value::Text("x".into())));
    assert_eq!(NOTE.from_json(&Json::Bool(true)), None);
    assert!(!NOTE.set(&mut a, Value::Text("x".repeat(MAX_TEXT + 1))));
}

#[test]
fn path_text_and_parsing_keep_the_home_page_and_wsl() {
    for s in ["Home", r"C:\Users", "wsl://Ubuntu/home", "This PC"] {
        let l = parse_path(s).unwrap();
        assert_eq!(parse_path(&path_text(&l)), Some(l), "{s}");
    }
    assert_eq!(parse_path("  "), None);
    assert_eq!(parse_path("relative\\path"), None);
}

#[test]
fn global_settings_apply_to_every_tab() {
    let mut a = app();
    a.dispatch(vec![Route { folder: Location::parse(r"C:\y").unwrap(), select: None }], OpenTarget::NewTab, Instant::now());
    assert!(a.tabs().iter().all(|t: &Tab| !t.view_spec.show_hidden));
    set(&mut a, "files.show_hidden", Value::Bool(true));
    assert!(a.tabs().iter().all(|t| t.view_spec.show_hidden));
    set(&mut a, "files.dotfiles", Value::Bool(false));
    assert!(a.tabs().iter().all(|t| t.view_spec.hide_dotfiles));
}

#[test]
fn switching_language_retranslates_the_status_message() {
    let mut a = app();
    set(&mut a, LANGUAGE, Value::Choice(1));
    a.status(S::ArchiveSelectToCompress.t());
    set(&mut a, LANGUAGE, Value::Choice(2));
    assert_eq!(a.tab().error.as_deref(), Some(S::ArchiveSelectToCompress.in_lang(Lang::Ru)));
    set(&mut a, LANGUAGE, Value::Choice(1));
    assert_eq!(a.tab().error.as_deref(), Some(S::ArchiveSelectToCompress.en()));
}

#[test]
fn single_click_modes_follow_the_pointer_and_old_booleans_still_load() {
    use super::ClickOpen::*;
    let opens: Vec<(bool, bool)> = [Never, Touch, Mouse, Always].iter().map(|m| (m.opens(false), m.opens(true))).collect();
    assert_eq!(opens, [(false, false), (false, true), (true, false), (true, true)]);
    let doc = parse(r#"{"files.single_click_files": true, "files.single_click_folders": false, "files.single_click_columns": "touch"}"#);
    assert!(doc.bad.is_empty(), "{:?}", doc.problems);
    let mut a = app();
    a.apply_doc(&doc);
    assert_eq!((a.single_click_files, a.single_click_folders, a.single_click_columns), (Always, Never, Touch));
    a.single_click_folders = Mouse;
    let text = render(&a.setting_values(), &Map::new(), None);
    assert!(text.contains(r#""files.single_click_folders": "mouse""#), "{text}");
}

#[test]
fn the_schema_is_written_unmarked_files_read_as_current_and_newer_ones_are_flagged() {
    let text = render(defaults(), &Map::new(), None);
    assert!(text.contains("\"schema\": 2"), "{text}");
    let doc = parse(&text);
    assert_eq!((doc.schema, doc.newer(), doc.values.len(), doc.unknown.len()), (SCHEMA, false, SETTINGS.len(), 0));
    let today = parse("{\"files.show_hidden\": true}");
    assert_eq!((today.schema, today.newer(), today.value("files.show_hidden")), (SCHEMA, false, Some(&Value::Bool(true))));
    let newer = parse("{\"schema\": 3, \"files.show_hidden\": true, \"future.key\": 1}");
    assert!(newer.newer() && !newer.problems.is_empty());
    assert!(render(defaults(), &newer.unknown, None).matches("\"schema\"").count() == 1);
}

#[test]
fn settings_migrate_step_by_step() {
    fn rename(m: &mut Map<String, Json>) {
        if let Some(v) = m.remove("old.key") {
            m.insert("mid.key".into(), v);
        }
    }
    fn bump(m: &mut Map<String, Json>) {
        if let Some(v) = m.remove("mid.key") {
            m.insert("new.key".into(), Json::from(v.as_i64().unwrap_or(0) + 1));
        }
    }
    let steps: [Migration; 2] = [rename, bump];
    let mut m: Map<String, Json> = serde_json::from_str("{\"old.key\": 1}").unwrap();
    assert!(migrate(&mut m, 1, 3, &steps));
    assert_eq!(m.get("new.key"), Some(&Json::from(2)));
    let mut m: Map<String, Json> = serde_json::from_str("{\"mid.key\": 5}").unwrap();
    assert!(migrate(&mut m, 2, 3, &steps));
    assert_eq!(m.get("new.key"), Some(&Json::from(6)));
    assert!(!migrate(&mut m, 0, 3, &steps));
    assert!(migrate(&mut m, 3, 3, &steps));
}

#[test]
fn the_interface_scale_multiplies_the_display_scale_for_layout_fonts_and_hits() {
    let mut a = app();
    a.on_event(&crate::input::Event::ScaleChanged(1.5), Instant::now());
    assert_eq!(get(&a, "appearance.scale"), Value::Number(100));
    let (row, title, font) = (a.layout.row_h, a.layout.title, a.ui_font(13.0).size_q);
    assert!(set(&mut a, "appearance.scale", Value::Number(200)));
    assert_eq!(a.scale, 3.0);
    assert!((a.layout.row_h - 2.0 * row).abs() <= 1.0, "{} vs {row}", a.layout.row_h);
    assert!((a.layout.title.h - 2.0 * title.h).abs() <= 1.0);
    assert_eq!(a.ui_font(13.0).size_q, 2 * font);
    assert!(a.layout.title.contains(title.x + 1.0, title.bottom() + 2.0));
    assert_eq!(a.hit_test(title.right() - 1.0, title.bottom() + 2.0), a.hit_test(title.right() - 1.0, 1.0));
    a.on_event(&crate::input::Event::ScaleChanged(1.0), Instant::now());
    assert_eq!(a.scale, 2.0);
    assert!(set(&mut a, "appearance.scale", Value::Number(110)));
    assert!((a.scale - 1.1).abs() < 1e-6);
    assert!(set(&mut a, "appearance.scale", Value::Number(70)));
    assert!(a.layout.row_h < row);
    assert!(!set(&mut a, "appearance.scale", Value::Number(250)));
    let doc = parse(r#"{"appearance.scale": 50}"#);
    assert_eq!(doc.bad, [index("appearance.scale").unwrap()]);
    a.apply_doc(&parse(r#"{"appearance.scale": 150}"#));
    assert_eq!(a.scale, 1.5);
}
