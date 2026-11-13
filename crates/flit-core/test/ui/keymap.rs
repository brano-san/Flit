use super::*;
use crate::input::Event;
use crate::location::Location;
use crate::ui::actions::{alt, ctrl, ctrl_alt, ctrl_shift, key, shift, Ctx};
use crate::ui::settings::{self, Value, SETTINGS};
use crate::ui::{Effect, MenuItem, Persisted, WindowCmd};
use serde_json::json;
use std::time::Duration;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

fn app() -> App {
    App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1100, 720), Instant::now())
}

fn press(a: &mut App, k: Key, m: Mods) {
    a.on_event(&Event::Key { key: k, mods: m, pressed: true }, Instant::now());
}

fn at(id: &str) -> usize {
    index(id).unwrap()
}

fn note(a: &App) -> String {
    a.tab().error.clone().unwrap_or_default()
}

#[test]
fn every_default_shortcut_reads_back_and_stays_assignable() {
    for a in ACTIONS {
        for &c in a.keys {
            assert_eq!(parse_chord(&chord_text(&c)), Some(c), "{}", chord_text(&c));
            assert_eq!(refusal(c), None, "{} {}", a.id, chord_text(&c));
        }
    }
    assert_eq!(parse_chord(" ctrl+SHIFT+p "), Some(ctrl_shift(Key::Char('P'))));
    assert_eq!(parse_chord("Ctrl++"), Some(ctrl(Key::Char('='))));
    assert_eq!(parse_chord("Ctrl+Insert"), Some(ctrl(Key::Insert)));
    assert_eq!(parse_chord("Escape"), Some(key(Key::Escape)));
    assert_eq!(parse_chord("f24"), Some(key(Key::F(24))));
    assert_eq!(parse_chord("Alt+Shift+x"), Some(Chord { key: Key::Char('X'), mods: Mods { ctrl: false, shift: true, alt: true } }));
    for bad in ["", "Ctrl+", "Ctrl+Ctrl+A", "F25", "F0", "Hyper+A", "AB", "Ctrl+Ж"] {
        assert_eq!(parse_chord(bad), None, "{bad}");
    }
    for refused in [key(Key::Char('A')), shift(Key::Char('A')), shift(Key::Char('1')), ctrl(Key::Home), Chord { key: Key::Other, mods: CTRL }] {
        assert!(refusal(refused).is_some(), "{}", chord_text(&refused));
    }
    for allowed in [ctrl(Key::Char('1')), ctrl(Key::Space), key(Key::Space)] {
        assert_eq!(refusal(allowed), None, "{} is an ordinary command key now", chord_text(&allowed));
    }
    assert_eq!(refusal(shift(Key::Tab)), None, "a default shortcut stays allowed");
    assert_eq!(refusal(alt(Key::Char('Q'))), None);
    assert_eq!(refusal(key(Key::Insert)), None);
    assert_eq!(defaults().owner(key(Key::BrowserBack)), None, "the Back key goes back unless a command takes it");
}

#[test]
fn the_file_keeps_unknown_ids_reports_bad_entries_and_resets_clashes() {
    let j = json!({
        "file.copy": ["Ctrl+C", "ctrl+ins", "Ctrl+C"],
        "nav.refresh": [],
        "future.command": ["Ctrl+Q"],
        "file.cut": "Ctrl+X",
        "file.paste": ["Ctrl+Shift+F9", "Banana"],
        "edit.undo": ["Ctrl+Shift+F9", 5],
        "edit.redo": ["Y"],
        "tab.new": ["Ctrl+G"],
        "view.hidden": ["Ctrl+G"],
        "select.all": ["Ctrl+T"]
    });
    let (k, problems) = parse(&j).unwrap();
    assert_eq!(k.keys(at("file.copy")), [ctrl(Key::Char('C')), ctrl(Key::Insert)]);
    assert!(k.keys(at("nav.refresh")).is_empty() && k.is_custom(at("nav.refresh")));
    assert_eq!(k.owner(key(Key::F(5))), None, "a command can have no shortcut at all");
    for id in ["file.cut", "file.paste", "edit.undo", "edit.redo", "tab.new", "view.hidden", "select.all"] {
        assert_eq!(k.keys(at(id)), ACTIONS[at(id)].keys, "{id} keeps its defaults");
    }
    let all = problems.join("\n");
    for part in ["shortcuts of file.cut in the settings file are not a list", "\"Banana\" in the shortcuts of file.paste is not a key", "5 in the shortcuts of edit.undo", "Y cannot be a shortcut of edit.redo", "Ctrl+G was given to both tab.new and view.hidden", "Ctrl+T was given to both tab.new and select.all"] {
        assert!(all.contains(part), "{part} in\n{all}");
    }
    let out = k.to_json();
    assert_eq!(out["future.command"], json!(["Ctrl+Q"]));
    assert!(out.get("file.copy").is_none(), "the defaults written out again are not custom");
    assert_eq!(out["nav.refresh"], json!([]));
    assert!(out.get("tab.new").is_none() && out.get("file.paste").is_none());
    assert_eq!(parse(&out), Some((k, Vec::new())));
    assert_eq!(parse(&json!({})).map(|(k, _)| k), Some(defaults()));
    assert!(parse(&json!(["Ctrl+C"])).is_none());
}

#[test]
fn commands_added_by_later_stages_take_part_in_remapping_and_clashes() {
    let toggle = at("theme.toggle");
    assert_eq!(defaults().owner(ctrl_alt(Key::Char('T'))), Some(toggle));
    for id in ["theme.light", "theme.dark", "theme.system", "help.docs"] {
        assert!(index(id).is_some(), "{id}");
    }
    let (k, problems) = parse(&json!({"file.copy": ["Ctrl+Alt+T"], "theme.light": ["Ctrl+Alt+L"]})).unwrap();
    assert_eq!(k.keys(toggle), [ctrl_alt(Key::Char('T'))]);
    assert_eq!(k.keys(at("file.copy")), [ctrl(Key::Char('C')), ctrl(Key::Insert)]);
    assert!(problems.iter().any(|p| p.contains("Ctrl+Alt+T was given to both file.copy and theme.toggle") || p.contains("Ctrl+Alt+T was given to both theme.toggle and file.copy")), "{problems:?}");
    let mut a = app();
    a.edit_keys(|m| m.assign(at("theme.light"), None, ctrl_alt(Key::Char('T'))));
    assert!(a.keymap().keys(toggle).is_empty(), "taking Ctrl+Alt+T leaves the theme toggle without a shortcut");
    assert_eq!(a.keymap().owner(ctrl_alt(Key::Char('T'))), Some(at("theme.light")));
}

#[test]
fn shortcuts_live_in_the_settings_file_and_travel_with_export_import_and_reset() {
    let mut a = app();
    a.edit_keys(|k| k.assign(at("view.hidden"), None, ctrl(Key::Char('G'))));
    let text = settings::render(&a.setting_values(), &Default::default(), None);
    assert!(text.contains("  \"keys\": {\n    \"view.hidden\": [\"Ctrl+H\",\"Ctrl+G\"]\n  }"), "{text}");
    let doc = settings::parse(&text);
    assert!(doc.problems.is_empty() && doc.bad.is_empty(), "{:?}", doc.problems);
    let mut b = app();
    b.apply_doc(&doc);
    assert_eq!(b.keymap(), a.keymap());
    let mut c = app();
    c.import_settings(&a.export_text());
    assert_eq!(c.keymap().keys(at("view.hidden")), [ctrl(Key::Char('H')), ctrl(Key::Char('G'))]);
    a.reset_settings();
    assert_eq!(a.keymap(), &defaults());
    let doc = settings::parse(r#"{"keys": {"file.copy": ["Q"], "later.thing": ["Ctrl+J"]}}"#);
    let mut d = app();
    d.apply_doc(&doc);
    assert!(note(&d).contains("Q cannot be a shortcut of file.copy"), "{}", note(&d));
    let again = settings::render(&d.setting_values(), &doc.unknown, None);
    assert!(again.contains("\"later.thing\": [\"Ctrl+J\"]"), "unknown commands survive a rewrite: {again}");
    let broken = settings::parse(r#"{"keys": ["Ctrl+C"]}"#);
    assert_eq!(broken.bad, [settings::index("keys").unwrap()]);
    let keys = &SETTINGS[settings::index("keys").unwrap()];
    assert!(keys.set(&mut d, Value::Keys(defaults())));
    assert!(!keys.set(&mut d, Value::Bool(true)));
}

#[test]
fn remapped_keys_run_their_new_commands_and_text_fields_keep_their_keys() {
    let mut a = app();
    let shown = a.tab().view_spec.show_hidden;
    a.edit_keys(|k| k.assign(at("view.hidden"), Some(ctrl(Key::Char('H'))), ctrl(Key::Char('G'))));
    press(&mut a, Key::Char('H'), CTRL);
    assert_eq!(a.tab().view_spec.show_hidden, shown, "the old key does nothing");
    press(&mut a, Key::Char('G'), CTRL);
    assert_eq!(a.tab().view_spec.show_hidden, !shown, "the new key runs the command");
    a.edit_keys(|k| k.assign(at("view.hidden"), None, ctrl(Key::Char('A'))));
    assert_eq!(a.keymap().owner(ctrl(Key::Char('A'))), Some(at("view.hidden")), "Ctrl+A moved from Select all");
    press(&mut a, Key::Char('F'), CTRL);
    for ch in "ab".chars() {
        a.on_event(&Event::Char(ch), Instant::now());
    }
    press(&mut a, Key::Char('A'), CTRL);
    press(&mut a, Key::Backspace, NO);
    assert_eq!(a.tab().filter.as_ref().unwrap().text(), "", "Ctrl+A and Backspace still edit the filter");
    assert_eq!(a.tab().view_spec.show_hidden, !shown);
}

#[test]
fn hints_in_menus_and_the_palette_follow_the_keymap() {
    let mut a = app();
    a.edit_keys(|k| k.assign(at("options.open"), Some(ctrl(Key::Char(','))), ctrl(Key::F(9))));
    a.open_main_menu();
    let menu = a.drain_effects().find_map(|e| if let crate::ui::Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).unwrap();
    assert!(menu.iter().any(|i| matches!(i, MenuItem::Command { label, keys, .. } if label == "Options" && keys == "Ctrl+F9")), "{menu:?}");
    a.open_palette();
    let p = a.nav.palette.as_ref().unwrap();
    let row = p.titles.iter().position(|t| t == "Options").unwrap();
    assert_eq!(p.keys[row], "Ctrl+F9");
    press(&mut a, Key::Escape, NO);
    press(&mut a, Key::F(9), CTRL);
    assert!(a.settings_open());
    press(&mut a, Key::Char(','), CTRL);
    assert!(a.settings_open(), "the old key no longer closes the window");
    press(&mut a, Key::F(9), CTRL);
    assert!(!a.settings_open(), "the new key closes it");
}

#[test]
#[ignore]
fn bench_keymap() {
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let keys = ('A'..='Z').chain('0'..='9').map(String::from).chain((1..=24).map(|n| format!("F{n}")));
    let mut chords: Vec<String> = keys.map(|k| format!("Ctrl+Shift+Alt+{k}")).collect();
    for m in ["Ctrl+Shift+", "Alt+Shift+", "Ctrl+Alt+", "Alt+"] {
        chords.extend((1..=24).map(|n| format!("{m}F{n}")));
    }
    let custom: serde_json::Map<String, Json> = ACTIONS.iter().zip(&chords).map(|(a, c)| (a.id.to_owned(), json!([c]))).collect();
    let entries = custom.len();
    let few = json!({"file.copy": ["Ctrl+C", "Ctrl+Ins"], "view.hidden": ["Ctrl+G"], "nav.refresh": []});
    let t = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(parse(&few));
    }
    let parse_few = t.elapsed() / 1000;
    let many = Json::Object(custom);
    let clashes = parse(&many).unwrap().1.len();
    let t = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(parse(&many));
    }
    let parse_many = t.elapsed() / 1000;
    let t = Instant::now();
    let built = Keymap::new(vec![None; ACTIONS.len()], Map::new(), false);
    let build = t.elapsed();
    let (remapped, _) = parse(&few).unwrap();
    let probes: Vec<Chord> = ACTIONS.iter().flat_map(|a| a.keys.iter().copied()).chain([ctrl(Key::Char('G')), ctrl(Key::Char('Q'))]).collect();
    let n = 2_000_000;
    let t = Instant::now();
    for k in 0..n {
        std::hint::black_box(remapped.owner(probes[k % probes.len()]));
    }
    let lookup = t.elapsed() / n as u32;
    let t = Instant::now();
    for k in 0..n {
        let c = probes[k % probes.len()];
        std::hint::black_box(crate::ui::actions::find(c.key, c.mods));
    }
    let linear = t.elapsed() / n as u32;
    let file = settings::render(settings::defaults(), &Default::default(), None);
    let without: String = file.lines().filter(|l| !l.contains("\"keys\"")).collect::<Vec<_>>().join("\n").replace(",\n}", "\n}");
    let with_few = file.replace("\"keys\": {}", &format!("\"keys\": {few}"));
    let mut files = [0.0; 3];
    for _ in 0..5 {
        for (slot, text) in files.iter_mut().zip([&without, &file, &with_few]) {
            let t = Instant::now();
            for _ in 0..1000 {
                std::hint::black_box(settings::parse(text));
            }
            *slot += us(t.elapsed() / 1000) / 5.0;
        }
    }
    eprintln!("BENCH settings file: without keys {:.1} us, with empty keys {:.1} us, with 3 custom commands {:.1} us", files[0], files[1], files[2]);
    let mut a = app();
    a.set_keymap(remapped);
    let mut events = Vec::new();
    for _ in 0..2000 {
        let t = Instant::now();
        press(&mut a, Key::Char('Q'), CTRL);
        events.push(t.elapsed());
    }
    events.sort();
    std::hint::black_box(built);
    eprintln!(
        "BENCH keymap: parse 3 entries {:.2} us, parse {} entries ({} problems) {:.1} us, default table {:.1} us; chord lookup {:.1} ns (linear scan of the registry {:.1} ns); unbound Ctrl+Q through on_event median {:.2} us",
        us(parse_few),
        entries,
        clashes,
        us(parse_many),
        us(build),
        lookup.as_secs_f64() * 1e9,
        linear.as_secs_f64() * 1e9,
        us(events[events.len() / 2])
    );
}

fn linux_app() -> App {
    let mut a = app();
    a.set_caps(crate::caps::Caps { unix_paths: true });
    a
}

#[test]
fn no_platform_table_gives_one_chord_to_two_commands() {
    for unix in [false, true] {
        let mut seen: Vec<(Ctx, Chord, &str)> = Vec::new();
        for a in ACTIONS {
            for &c in a.defaults(unix) {
                if let Some((_, _, other)) = seen.iter().find(|(x, k, _)| *x == a.ctx && *k == c) {
                    panic!("unix {unix}: {other} and {} share {}", a.id, chord_text(&c));
                }
                seen.push((a.ctx, c, a.id));
                assert_eq!(parse_chord(&chord_text(&c)), Some(c));
            }
        }
        let k = defaults_for(unix);
        for (i, a) in ACTIONS.iter().enumerate().filter(|(_, a)| a.rebindable()) {
            assert_eq!(k.keys(i), a.defaults(unix), "{}", a.id);
            for c in a.defaults(unix) {
                assert_eq!(k.owner(*c), Some(i), "unix {unix}: {}", chord_text(c));
            }
        }
    }
}

#[test]
fn linux_defaults_add_the_chords_file_manager_users_expect() {
    let k = defaults_for(true);
    for (id, c) in [
        ("quick_access.toggle", key(Key::F(9))),
        ("quick_access.toggle", ctrl(Key::Char('B'))),
        ("nav.home", alt(Key::Home)),
        ("edit.redo", ctrl_shift(Key::Char('Z'))),
        ("edit.redo", ctrl(Key::Char('Y'))),
        ("app.quit", ctrl(Key::Char('Q'))),
        ("folder.bookmark", ctrl(Key::Char('D'))),
        ("tab.select_1", alt(Key::Char('1'))),
        ("tab.select_8", alt(Key::Char('8'))),
        ("tab.select_last", alt(Key::Char('9'))),
        ("view.layout.icons_m", ctrl(Key::Char('1'))),
        ("view.layout.list", ctrl(Key::Char('2'))),
        ("view.layout.details", ctrl(Key::Char('3'))),
        ("select.invert", ctrl_shift(Key::Char('I'))),
    ] {
        assert!(k.runs(c.key, c.mods, id), "{id} {}", chord_text(&c));
    }
    assert_eq!(k.owner(ctrl(Key::Char('D'))), index("folder.bookmark"));
    assert_eq!(k.owner(ctrl_shift(Key::Char('I'))), index("select.invert"));
    assert_eq!(k.text_of("file.new_item"), "");
    assert_eq!(k.text_of("tab.select_1"), "Alt+1");
    assert!(k.all_text("tab.select_1").is_empty() || !k.all_text("tab.select_1").contains("Ctrl"));
    assert!(k.shown(at("tab.select_1")).contains(&"Alt+Keypad1".to_owned()));
}

#[test]
fn windows_defaults_are_untouched_by_the_linux_table() {
    let k = defaults_for(false);
    assert_eq!(k.owner(ctrl(Key::Char('1'))), index("tab.select_1"));
    assert_eq!(k.owner(ctrl(Key::Char('D'))), index("list.page_down"));
    assert_eq!(k.owner(ctrl_shift(Key::Char('I'))), index("file.new_item"));
    assert_eq!(k.owner(key(Key::F(9))), None);
    assert_eq!(k.owner(alt(Key::Home)), None);
    assert_eq!(k.owner(ctrl(Key::Char('Q'))), None);
    assert_eq!(k.text_of("folder.bookmark"), "");
    assert_eq!(k.text_of("edit.redo"), "Ctrl+Y, Ctrl+Shift+Z");
}

#[test]
fn a_linux_app_runs_its_own_defaults_and_resets_to_them() {
    let mut a = linux_app();
    assert_eq!(a.keymap(), &defaults_for(true));
    assert!(a.keymap().unix());
    for _ in 0..2 {
        a.run_action("tab.duplicate", Instant::now());
    }
    press(&mut a, Key::Char('2'), Mods { alt: true, ..NO });
    assert_eq!(a.cur, 1);
    press(&mut a, Key::Char('9'), Mods { alt: true, ..NO });
    assert_eq!(a.cur, 2);
    press(&mut a, Key::Char('1'), CTRL);
    assert_eq!(a.cur, 2, "Ctrl+1 is a view mode on Linux");
    press(&mut a, Key::F(9), NO);
    assert!(!a.nav.sidebar.state.sidebar);
    press(&mut a, Key::Char('Q'), CTRL);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Window(WindowCmd::Quit))), "Ctrl+Q quits every window on Linux");
    a.edit_keys(|k| k.assign(at("nav.home"), None, ctrl(Key::Char('G'))));
    assert!(a.keymap().is_custom(at("nav.home")));
    a.restore_all_keys();
    assert_eq!(a.keymap(), &defaults_for(true));
    assert_eq!(a.keymap().keys(at("nav.home")), [alt(Key::Home)]);
}

#[test]
fn saved_keymaps_are_read_against_the_platform_they_load_on() {
    let j = json!({ "tab.select_1": ["Alt+1"], "tab.select_2": ["Ctrl+2"], "view.layout.list": ["Ctrl+2"] });
    let (windows, problems) = parse_for(&j, false).unwrap();
    assert_eq!(windows.keys(at("tab.select_1")), [alt(Key::Char('1'))]);
    assert_eq!(problems.len(), 1, "{problems:?}");
    let (linux, problems) = parse_for(&json!({ "tab.select_1": ["Alt+1"], "folder.bookmark": ["Ctrl+D"], "nav.home": ["Alt+Home"] }), true).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    assert!(!linux.is_custom(at("tab.select_1")), "equal to the Linux default, so not custom");
    assert!(!linux.is_custom(at("folder.bookmark")));
    let (linux, problems) = parse_for(&json!({ "view.layout.list": ["Ctrl+2"], "tab.select_2": ["Alt+2"] }), true).unwrap();
    assert!(problems.is_empty() && linux.keys(at("view.layout.list")) == [ctrl(Key::Char('2'))], "{problems:?}");
    let mut a = app();
    a.set_keymap(parse_for(&json!({ "nav.home": ["Ctrl+G"] }), false).unwrap().0);
    a.set_caps(crate::caps::Caps { unix_paths: true });
    assert_eq!(a.keymap().keys(at("nav.home")), [ctrl(Key::Char('G'))], "a user binding survives the platform switch");
    assert_eq!(a.keymap().keys(at("tab.select_1")), [alt(Key::Char('1'))]);
    assert_eq!(a.keymap().to_json(), json!({ "nav.home": ["Ctrl+G"] }));
}

#[test]
fn the_shortcut_editor_restores_the_platform_default() {
    let mut a = linux_app();
    let i = at("nav.home");
    a.edit_keys(|k| k.assign(i, None, ctrl(Key::Char('G'))));
    a.edit_keys(|k| k.restore(i));
    assert_eq!(a.keymap().keys(i), [alt(Key::Home)]);
    a.edit_keys(|k| k.assign(i, None, ctrl(Key::Char('G'))));
    a.edit_shortcuts(i);
    assert_eq!(a.nav.palette.as_ref().unwrap().titles.len(), 5, "two chords, Add, Restore and Reset rows");
}
