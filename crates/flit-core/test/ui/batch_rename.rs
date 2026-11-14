use super::*;
use crate::input::{Event, Key, Mods};
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::provider::{FileOp, ProviderRequest};
use crate::sort::{self, CaseMode, SortKeys};
use crate::ui::{Effect, Persisted};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn spec(find: &str, replace: &str) -> Spec {
    Spec { find: find.into(), replace: replace.into(), case: false, numbering: Numbering::None, first: 1 }
}

fn names(items: &[(&str, bool)], s: &Spec) -> Vec<String> {
    let items: Vec<(&[u8], bool)> = items.iter().map(|(n, d)| (n.as_bytes(), *d)).collect();
    new_names(&items, s).into_iter().map(|n| String::from_utf8(n).unwrap()).collect()
}

fn bytes(v: &[&str]) -> Vec<Vec<u8>> {
    v.iter().map(|s| s.as_bytes().to_vec()).collect()
}

#[test]
fn find_and_replace_works_on_the_name_and_keeps_the_extension() {
    let items = [("IMG_001.JPG", false), ("img_002.jpg", false), ("Img folder.old", true)];
    assert_eq!(names(&items, &spec("img", "Photo")), ["Photo_001.JPG", "Photo_002.jpg", "Photo folder.old"]);
    let mut exact = spec("img", "Photo");
    exact.case = true;
    assert_eq!(names(&items, &exact), ["IMG_001.JPG", "Photo_002.jpg", "Img folder.old"], "match case leaves other cases alone");
    assert_eq!(names(&[("report.jpg", false)], &spec("jpg", "png")), ["report.jpg"], "the extension is not searched");
    assert_eq!(names(&[("Ärger ärger.txt", false)], &spec("ÄRGER", "x")), ["x x.txt"]);
    assert_eq!(names(&items, &spec("", "zzz")), ["IMG_001.JPG", "img_002.jpg", "Img folder.old"], "nothing to find changes nothing");
}

#[test]
fn numbers_are_padded_to_the_widest_and_go_after_or_before_the_name() {
    let items: Vec<(String, bool)> = (0..12).map(|k| (format!("f{k}.txt"), false)).collect();
    let items: Vec<(&str, bool)> = items.iter().map(|(n, d)| (n.as_str(), *d)).collect();
    let mut s = spec("", "");
    s.numbering = Numbering::After;
    let out = names(&items, &s);
    assert_eq!((out[0].as_str(), out[11].as_str()), ("f0 01.txt", "f11 12.txt"));
    s.numbering = Numbering::Before;
    s.first = 7;
    assert_eq!(names(&items[..2], &s), ["7 f0.txt", "8 f1.txt"]);
    let mut wipe = spec("photo", "");
    wipe.numbering = Numbering::After;
    assert_eq!(names(&[("photo.jpg", false)], &wipe), ["1.jpg"], "an emptied name is just the number");
}

#[test]
fn bad_duplicate_or_taken_names_are_refused() {
    let old = bytes(&["a.txt", "b.txt"]);
    let free = |_: usize, _: &[u8]| false;
    assert_eq!(problem(&old, &bytes(&["c.txt", "c.txt"]), false, free), Some(Problem::Twice(1)));
    assert_eq!(problem(&old, &bytes(&["c.txt", "C.TXT"]), true, free), Some(Problem::Twice(1)), "Windows names ignore case");
    assert_eq!(problem(&old, &bytes(&["c.txt", "C.TXT"]), false, free), None, "Linux names do not");
    assert_eq!(problem(&old, &bytes(&["", "b.txt"]), false, free), Some(Problem::Empty(0)));
    assert_eq!(problem(&old, &bytes(&["a/b", "b.txt"]), false, free), Some(Problem::Invalid(0)));
    assert_eq!(problem(&old, &bytes(&["a:b", "b.txt"]), true, free), Some(Problem::Invalid(0)));
    assert_eq!(problem(&old, &bytes(&["a:b", "b.txt"]), false, free), None);
    assert_eq!(problem(&old, &bytes(&["d.txt", "b.txt"]), false, |_, n| n == b"d.txt"), Some(Problem::Taken(0)));
    assert_eq!(problem(&old, &bytes(&["A.txt", "b.txt"]), true, |_, n| n == b"a.txt"), None, "changing only the case is no clash");
}

#[test]
fn renames_into_each_others_names_run_in_a_safe_order_and_swaps_are_refused() {
    let old = bytes(&["a", "b", "c"]);
    assert_eq!(order(&old, &bytes(&["b", "c", "d"]), vec![0, 1, 2]), Some(vec![2, 1, 0]));
    assert_eq!(order(&old, &bytes(&["b", "a", "c"]), vec![0, 1]), None);
    assert_eq!(order(&old, &bytes(&["x", "b", "c"]), vec![0]), Some(vec![0]));
}

fn app_with(names: &[&str]) -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in names {
        let i = listing.push(n.as_bytes(), Meta { size: 1, modified: 1, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = t.view_spec;
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::from_millis(1) };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    app
}

fn select_all(app: &mut App) {
    app.on_event(&Event::Key { key: Key::Char('A'), mods: Mods { ctrl: true, ..Mods::default() }, pressed: true }, Instant::now());
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_event(&Event::Char(c), Instant::now());
    }
}

fn renamed(app: &mut App) -> Option<Vec<(String, String)>> {
    app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Op { op: FileOp::MoveTo(pairs), .. }, .. } => Some(pairs.iter().map(|(a, b)| (String::from_utf8_lossy(a.file_name()).into_owned(), String::from_utf8_lossy(b.file_name()).into_owned())).collect()),
        _ => None,
    })
}

#[test]
fn the_dialog_previews_the_new_names_and_renames_the_selection() {
    let mut app = app_with(&["IMG_1.jpg", "IMG_2.jpg", "notes.txt"]);
    select_all(&mut app);
    assert!(app.run_action("file.batch_rename", Instant::now()));
    assert_eq!(app.asking(), Some("batch_rename"));
    typed(&mut app, "img");
    app.on_event(&Event::Key { key: Key::Tab, mods: Mods::default(), pressed: true }, Instant::now());
    typed(&mut app, "Trip");
    let f = app.jobs.form.as_ref().unwrap();
    let lines: Vec<&str> = f.controls.iter().filter_map(|c| if let Control::Line(t) = c { Some(t.as_str()) } else { None }).collect();
    assert_eq!(&lines[..3], ["IMG_1.jpg → Trip_1.jpg", "IMG_2.jpg → Trip_2.jpg", "notes.txt (unchanged)"]);
    assert_eq!((f.note.as_str(), f.error), ("2 items will be renamed", false));
    app.on_event(&Event::Key { key: Key::Enter, mods: Mods::default(), pressed: true }, Instant::now());
    assert_eq!(renamed(&mut app).unwrap_or_default(), [("IMG_1.jpg".into(), "Trip_1.jpg".into()), ("IMG_2.jpg".into(), "Trip_2.jpg".into())]);
    assert!(app.jobs.form.is_none());
}

#[test]
fn a_clash_keeps_the_dialog_open_and_renames_nothing() {
    let mut app = app_with(&["a.txt", "b.txt", "c.txt"]);
    select_all(&mut app);
    app.batch_rename_dialog();
    typed(&mut app, "a");
    app.on_event(&Event::Key { key: Key::Tab, mods: Mods::default(), pressed: true }, Instant::now());
    typed(&mut app, "b");
    assert!(app.jobs.form.as_ref().unwrap().error);
    app.on_event(&Event::Key { key: Key::Enter, mods: Mods::default(), pressed: true }, Instant::now());
    let f = app.jobs.form.as_ref().expect("the dialog stays open");
    assert_eq!((f.note.as_str(), f.error), ("Two items would be named b.txt", true));
    assert!(renamed(&mut app).is_none());
    app.on_event(&Event::Key { key: Key::Escape, mods: Mods::default(), pressed: true }, Instant::now());
    assert!(app.jobs.form.is_none() && renamed(&mut app).is_none());
}
