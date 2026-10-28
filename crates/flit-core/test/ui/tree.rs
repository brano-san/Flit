use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::layouts::ZOOM_TREE;
use crate::ui::Persisted;
use std::time::{Duration, Instant};

fn app() -> App {
    App::new(Persisted { path: Location::parse(r"C:\Work").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now())
}

fn frame(app: &mut App) {
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
}

fn meta(dir: bool) -> Meta {
    Meta { attrs: if dir { attr::DIR } else { 0 }, size: 10, ..Default::default() }
}

fn snapshot(gen: u64, path: &Location, view: ViewSpec, top: &[&str], nested: &[(&str, &str)]) -> DirSnapshot {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in top {
        let i = listing.push(n.as_bytes(), meta(!n.contains('.')));
        keys.push(listing.name(i));
    }
    for (folder, n) in nested {
        let i = listing.push_nested(n.as_bytes(), meta(!n.contains('.')), 0, folder.as_bytes());
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    DirSnapshot { gen, path: path.path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO }
}

fn deliver(app: &mut App, top: &[&str], nested: &[(&str, &str)]) {
    let t = app.tab();
    let snap = snapshot(t.gen, t.location(), t.view_spec, top, nested);
    t.slot.publish(Arc::new(snap));
    app.poll();
    frame(app);
}

fn resort(app: &mut App) {
    loop {
        let job = app.drain_effects().filter_map(|e| if let Effect::Resort { tab, gen, view, snap } = e { Some((tab, gen, view, snap)) } else { None }).next_back();
        let Some((tab, gen, view, snap)) = job else { break };
        let mut order = Vec::new();
        sort::sort(&snap.listing, &snap.keys, view, &mut order);
        app.on_resorted(tab, gen, view, snap.listing.len(), order);
    }
    frame(app);
}

fn rows(app: &App) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.rows().iter().map(|&i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn focus(app: &mut App, name: &str) {
    let v = app.tab_mut().view.as_mut().unwrap();
    let i = (0..v.listing().len() as u32).find(|&i| v.listing().name(i) == name.as_bytes()).unwrap();
    v.clear_selection();
    v.set_selected(i, true);
    v.focus = Some(i);
}

fn key(app: &mut App, key: Key, shift: bool) {
    app.on_event(&Event::Key { key, mods: Mods { shift, ..Mods::default() }, pressed: true }, Instant::now());
    frame(app);
}

fn answer_probes(app: &mut App, children: &[(&str, &[&str])]) {
    let view = app.tab().view_spec;
    for p in &app.tab().tree.probes {
        let rel = String::from_utf8_lossy(&p.rel).into_owned();
        let names = children.iter().find(|c| c.0 == rel).map_or(&[][..], |c| c.1);
        let snap = snapshot(p.gen, &Location::parse(r"C:\Work").unwrap(), ViewSpec { tree: false, deep: false, ..view }, names, &[]);
        p.slot.publish(Arc::new(snap));
    }
    app.poll();
    frame(app);
}

fn tree_app() -> App {
    let mut app = app();
    deliver(&mut app, &["alpha", "beta", "z.txt"], &[]);
    assert!(app.run_action("view.layout.tree", Instant::now()));
    resort(&mut app);
    assert_eq!(app.tab().mode, LayoutMode::Tree);
    assert_eq!(app.tab().zoom, ZOOM_TREE);
    app
}

#[test]
fn opening_a_folder_in_the_tree_lists_it_and_shows_its_children_under_it() {
    let mut app = tree_app();
    app.drain_effects().for_each(drop);
    focus(&mut app, "alpha");
    key(&mut app, Key::Right, false);
    let listed = app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { ref path, view, .. }, .. } if path.as_bytes().ends_with(b"alpha") && !view.deep));
    assert!(listed, "the folder is listed on its own");
    answer_probes(&mut app, &[("alpha", &["inner", "a.txt"])]);
    assert_eq!(rows(&app), ["alpha", "inner", "a.txt", "beta", "z.txt"]);
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!(v.depth(v.rows()[1]), 1);
    key(&mut app, Key::Left, false);
    assert_eq!(rows(&app), ["alpha", "beta", "z.txt"], "Left closes the open folder");
    key(&mut app, Key::Right, false);
    assert_eq!(rows(&app), ["alpha", "inner", "a.txt", "beta", "z.txt"], "children load once");
    focus(&mut app, "a.txt");
    key(&mut app, Key::Left, false);
    assert_eq!(rows(&app), ["alpha", "beta", "z.txt"], "Left on a child closes its parent");
    assert_eq!(app.tab().view.as_ref().unwrap().focus.map(|i| app.tab().view.as_ref().unwrap().listing().name(i).to_vec()), Some(b"alpha".to_vec()));
}

#[test]
fn open_all_expands_every_level_and_close_all_folds_them() {
    let mut app = tree_app();
    focus(&mut app, "alpha");
    assert!(app.run_action("tree.open_all", Instant::now()));
    answer_probes(&mut app, &[("alpha", &["inner"])]);
    answer_probes(&mut app, &[("alpha\\inner", &["deep.txt"])]);
    assert_eq!(rows(&app), ["alpha", "inner", "deep.txt", "beta", "z.txt"]);
    focus(&mut app, "alpha");
    assert!(app.run_action("tree.close_all", Instant::now()));
    assert_eq!(rows(&app), ["alpha", "beta", "z.txt"]);
    assert!(app.tab().tree.open.is_empty());
}

#[test]
fn clicking_the_chevron_toggles_the_folder() {
    let mut app = tree_app();
    let cr = app.chevron_rect(0).expect("folders have a chevron");
    let (x, y) = (cr.x + cr.w / 2.0, cr.y + cr.h / 2.0);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
    answer_probes(&mut app, &[("alpha", &["b.txt"])]);
    assert_eq!(rows(&app), ["alpha", "b.txt", "beta", "z.txt"]);
    assert!(app.chevron_rect(1).is_none(), "files have no chevron");
}

#[test]
fn subfolders_mode_lists_nested_items_and_marks_the_tab() {
    let mut app = app();
    deliver(&mut app, &["alpha", "z.txt"], &[]);
    app.drain_effects().for_each(drop);
    assert!(app.run_action("subfolders.toggle", Instant::now()));
    let deep = app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { view, .. }, .. } if view.deep));
    assert!(deep, "the folder is listed with its subfolders");
    assert!(app.tab().title().ends_with(" [...]"));
    deliver(&mut app, &["alpha", "z.txt"], &[("alpha", "a.txt"), ("alpha", "sub"), ("alpha\\sub", "s.txt")]);
    let mut shown = rows(&app);
    shown.sort();
    assert_eq!(shown, ["a.txt", "alpha", "s.txt", "sub", "z.txt"]);
    let c = app.tab().view.as_ref().unwrap().all_counts();
    assert_eq!((c.dirs, c.files), (2, 3));
    assert!(app.run_action("subfolders.toggle", Instant::now()));
    resort(&mut app);
    assert_eq!(rows(&app), ["alpha", "z.txt"], "nested items hide without a reload");
    assert!(!app.tab().title().ends_with(" [...]"));
}

#[test]
fn folders_and_files_can_be_hidden_separately() {
    let mut app = app();
    deliver(&mut app, &["alpha", "beta", "z.txt"], &[]);
    assert!(app.run_action("view.toggle_folders", Instant::now()));
    resort(&mut app);
    assert_eq!(rows(&app), ["z.txt"]);
    assert!(app.run_action("view.toggle_files", Instant::now()));
    resort(&mut app);
    assert!(rows(&app).is_empty());
    assert!(app.run_action("view.toggle_folders", Instant::now()));
    resort(&mut app);
    assert_eq!(rows(&app), ["alpha", "beta"]);
    let shown = app.tab().view.as_ref().unwrap().shown_counts();
    assert_eq!((shown.dirs, shown.files), (2, 0));
}
