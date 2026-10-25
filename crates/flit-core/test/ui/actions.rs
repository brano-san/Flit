use super::*;
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::sort::{self, CaseMode, SortKeys};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::{Persisted, Places};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn chords_are_unique_and_printable() {
    let mut seen: Vec<Chord> = Vec::new();
    for a in ACTIONS {
        for c in a.keys {
            assert!(!seen.contains(c), "{} reuses {}", a.id, chord_text(c));
            seen.push(*c);
            assert_eq!(find(c.key, c.mods).map(|f| f.id), Some(a.id));
        }
    }
    let mut ids: Vec<&str> = ACTIONS.iter().map(|a| a.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), ACTIONS.len());
    assert_eq!(chord_text(&ctrl_shift(Key::Char('P'))), "Ctrl+Shift+P");
    assert_eq!(chord_text(&alt(Key::Left)), "Alt+Left");
    assert_eq!(chord_text(&key(Key::F(5))), "F5");
    let refresh = ACTIONS.iter().find(|a| a.id == "nav.refresh").unwrap();
    assert_eq!(keys_text(refresh), "F5, Ctrl+R");
    assert_eq!(find(Key::Char('T'), Mods { ctrl: true, shift: false, alt: true }).map(|a| a.id), None, "AltGr is not Ctrl");
}

#[test]
fn shortcuts_of_other_features_are_registered() {
    for c in [ctrl(Key::Char('E')), ctrl_shift(Key::Char('F'))] {
        assert_eq!(find(c.key, c.mods).map(|a| a.id), Some("search.open"), "{}", chord_text(&c));
    }
    let file_ops = [
        (ctrl(Key::Char('C')), "file.copy"),
        (ctrl(Key::Char('X')), "file.cut"),
        (ctrl(Key::Char('V')), "file.paste"),
        (key(Key::F(2)), "file.rename"),
        (ctrl_shift(Key::Char('N')), "file.new_folder"),
        (ctrl(Key::Char('Z')), "edit.undo"),
        (ctrl(Key::Char('Y')), "edit.redo"),
        (alt(Key::Enter), "item.properties"),
        (shift(Key::F(10)), "item.context_menu"),
        (key(Key::Apps), "item.context_menu"),
        (ctrl_shift(Key::Char('V')), "file.paste_into"),
        (ctrl_shift(Key::Char('I')), "file.new_item"),
        (ctrl_alt(Key::Char('C')), "item.copy_quoted_path"),
        (ctrl(Key::Char('`')), "app.terminal"),
        (ctrl_shift(Key::Char('`')), "app.terminal_admin"),
    ];
    for (c, id) in file_ops {
        assert_eq!(find(c.key, c.mods).map(|a| a.id), Some(id), "{}", chord_text(&c));
    }
    assert_eq!(chord_text(&key(Key::Apps)), "Menu");
    assert_eq!(chord_text(&ctrl_shift(Key::Char('`'))), "Ctrl+Shift+`");
    assert_eq!(chord_text(&ctrl_alt(Key::Char('C'))), "Ctrl+Alt+C");
}

#[test]
fn every_action_runs_headless() {
    for a in ACTIONS {
        let now = Instant::now();
        let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (900, 500), now);
        app.set_places(Arc::new(Places::default()));
        let t = app.tab();
        let mut listing = Listing::default();
        let mut keys = SortKeys::new(CaseMode::Insensitive);
        for n in ["a.txt", "b.txt"] {
            let i = listing.push(n.as_bytes(), Meta::default());
            keys.push(listing.name(i));
        }
        let mut order = Vec::new();
        sort::sort(&listing, &keys, t.view_spec, &mut order);
        let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: t.view_spec, done: true, error: None, elapsed: Duration::ZERO };
        t.slot.publish(Arc::new(snap));
        app.poll();
        (a.run)(&mut app, now);
        app.frame(&mut Gfx::new(Mono, 1024), now);
        assert!(!app.tabs().is_empty(), "{}", a.id);
    }
}

fn launched(app: &mut App) -> Vec<crate::provider::Launch> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: crate::provider::ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect()
}

fn press(app: &mut App, c: Chord) {
    app.on_event(&crate::input::Event::Key { key: c.key, mods: c.mods, pressed: true }, Instant::now());
}

fn bare_app() -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (900, 500), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

#[test]
fn help_and_log_shortcuts_open_the_documentation_the_log_file_and_its_folder() {
    use crate::path::FsPath;
    use crate::provider::{Launch, ProviderRequest};
    let mut app = bare_app();
    press(&mut app, key(Key::F(1)));
    assert_eq!(launched(&mut app), [Launch::Run { program: "https://github.com/brano-san/Flit#readme".into(), args: String::new(), dir: None }]);
    press(&mut app, ctrl(Key::Char('.')));
    assert!(app.drain_effects().all(|e| !matches!(e, Effect::Provider { req: ProviderRequest::Open { .. }, .. })), "no log folder, nothing opens");
    assert_eq!(app.tab().error.as_deref(), Some("Flit has no log folder in this session"));
    app.about.logs = FsPath::parse(r"C:\Users\me\AppData\Local\Flit\logs");
    press(&mut app, ctrl(Key::Char('.')));
    let opened: Vec<FsPath> = app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } = e { Some(path) } else { None }).collect();
    assert_eq!(opened, [FsPath::parse(r"C:\Users\me\AppData\Local\Flit\logs\flit.log").unwrap()]);
    press(&mut app, ctrl_shift(Key::Char('.')));
    assert_eq!((app.tabs().len(), app.current_location().display().into_owned()), (2, r"C:\Users\me\AppData\Local\Flit\logs".to_owned()));
    for (id, url) in super::super::settings_page::LINKS {
        assert!(app.run_action(id, Instant::now()), "{id}");
        assert_eq!(launched(&mut app), [Launch::Run { program: url.into(), args: String::new(), dir: None }], "{id}");
        assert!(url.starts_with("https://github.com/brano-san/Flit"), "{url}");
    }
}

#[test]
fn shell_folders_flit_cannot_show_go_to_file_explorer_with_a_note() {
    use crate::provider::Launch;
    let mut app = bare_app();
    let open = crate::open::Open { paths: vec![b"shell:ControlPanelFolder".to_vec()], ..Default::default() };
    app.open(&open, &|_| None, Instant::now());
    assert_eq!(launched(&mut app), [Launch::Run { program: "explorer.exe".into(), args: "\"shell:ControlPanelFolder\"".into(), dir: None }]);
    assert_eq!(app.tab().error.as_deref(), Some("Flit cannot show shell:ControlPanelFolder, so it opens in File Explorer"));
    assert_eq!(app.tabs().len(), 1);
    let known = crate::open::Open { paths: vec![b"shell:Downloads".to_vec()], ..Default::default() };
    app.open(&known, &|_| Location::parse(r"C:\Users\me\Downloads"), Instant::now());
    assert!(launched(&mut app).is_empty(), "folders with a file system path still open in Flit");
}
