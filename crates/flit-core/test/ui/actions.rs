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
fn shortcuts_reserved_for_other_features_stay_free() {
    for c in [ctrl(Key::Char('E')), ctrl_shift(Key::Char('F'))] {
        assert!(find(c.key, c.mods).is_none(), "{} is reserved", chord_text(&c));
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
    ];
    for (c, id) in file_ops {
        assert_eq!(find(c.key, c.mods).map(|a| a.id), Some(id), "{}", chord_text(&c));
    }
    assert_eq!(chord_text(&key(Key::Apps)), "Menu");
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
