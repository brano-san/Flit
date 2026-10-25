use super::*;
use crate::input::{Event, Key, Mods};
use crate::library::Library;
use crate::listing::{attr, DirSnapshot, ListingBuilder, Meta};
use crate::provider::{FileOp, ProviderRequest};
use crate::sort::{self, CaseMode};
use crate::ui::{DragEvent, Persisted, SearchRequest};
use std::sync::Arc;
use std::time::Duration;

const LIB: &str = r"C:\Users\u\AppData\Roaming\Microsoft\Windows\Libraries\Documents.library-ms";

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn documents(save: &str) -> Library {
    Library { name: "Documents".into(), path: p(LIB), folders: vec![p(r"C:\Users\u\Documents"), p(r"D:\Docs")], save: Some(p(save)) }
}

fn places(lib: Library) -> Arc<Places> {
    Arc::new(Places { libraries: vec![lib], ..Default::default() })
}

fn library_app() -> App {
    let mut app = App::new(Persisted { path: Location::Local(p(LIB)), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    app.set_places(places(documents(r"C:\Users\u\Documents")));
    let t = app.tab();
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    crate::library::push(&mut b, b"cv.docx", Meta::default(), &p(r"C:\Users\u\Documents"));
    crate::library::push(&mut b, b"Taxes", Meta { attrs: attr::DIR, ..Default::default() }, &p(r"D:\Docs"));
    let (listing, keys) = (b.listing, b.keys);
    let mut order = Vec::new();
    sort::sort(&listing, &keys, t.view_spec, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: p(LIB), listing, keys, order, view: t.view_spec, done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    app
}

fn key(app: &mut App, k: Key) {
    app.on_event(&Event::Key { key: k, mods: Mods::default(), pressed: true }, Instant::now());
}

fn library_ops(app: &mut App) -> Vec<LibraryOp> {
    app.drain_effects().filter_map(|e| if let Effect::Library(op) = e { Some(op) } else { None }).collect()
}

#[test]
fn a_library_lists_its_folders_and_saves_into_the_default_location() {
    let mut app = library_app();
    assert_eq!(app.tab().crumbs.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["Libraries", "Documents"]);
    assert_eq!(app.tab().title(), "Documents");
    let t = app.tab();
    let v = t.view.as_ref().unwrap();
    let names: Vec<Location> = v.rows().iter().map(|&i| t.entry_location(i)).collect();
    assert_eq!(names, [Location::Local(p(r"D:\Docs\Taxes")), Location::Local(p(r"C:\Users\u\Documents\cv.docx"))]);
    assert_eq!(app.shell_dir(), Ok(p(r"C:\Users\u\Documents")));
    app.set_places(places(documents(r"D:\Docs")));
    assert_eq!(app.shell_dir(), Ok(p(r"D:\Docs")), "the default save location follows the library");
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. }, .. })), "same folders, no reload");
    let mut more = documents(r"D:\Docs");
    more.folders.push(p(r"E:\Shared"));
    app.set_places(places(more));
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. }, .. })), "new folders reload the library");
    assert!(app.available("library.locations"));
    assert!(!app.available("app.terminal"));
}

#[test]
fn entries_belong_to_their_own_folder() {
    let mut app = library_app();
    let t = app.tab();
    assert_eq!(t.entry_dir(t.view.as_ref().unwrap().rows()[0]), Location::Local(p(r"D:\Docs")));
    app.set_places(places(Library { save: None, ..documents(r"C:\x") }));
    assert_eq!(app.shell_dir(), Ok(p(r"C:\Users\u\Documents")), "without a default the first folder takes new files");
    app.set_places(places(Library { save: None, folders: Vec::new(), ..documents(r"C:\x") }));
    assert!(app.shell_dir().is_err(), "a library without folders has nowhere to save");
}

#[test]
fn library_dialog_sets_the_save_location_removes_and_adds_folders() {
    let mut app = library_app();
    assert!(app.run_action("library.locations", Instant::now()));
    let pal = app.nav.palette.as_ref().unwrap();
    assert_eq!(pal.titles, [r"C:\Users\u\Documents", r"D:\Docs", "Add a folder…"]);
    assert_eq!(pal.keys[0], "Default save location");
    key(&mut app, Key::Down);
    key(&mut app, Key::Enter);
    assert_eq!(library_ops(&mut app), [LibraryOp::SetSave { library: p(LIB), folder: p(r"D:\Docs") }]);
    assert!(app.run_action("library.locations", Instant::now()));
    key(&mut app, Key::Delete);
    assert_eq!(library_ops(&mut app), [LibraryOp::Remove { library: p(LIB), folder: p(r"C:\Users\u\Documents") }]);
    assert!(app.run_action("library.locations", Instant::now()));
    key(&mut app, Key::Up);
    key(&mut app, Key::Enter);
    assert_eq!(app.nav.palette.as_ref().map(|p| p.pick.clone()), Some(Pick::LibraryAdd(p(LIB))));
    app.on_paste(r"E:\Shared");
    key(&mut app, Key::Enter);
    assert_eq!(library_ops(&mut app), [LibraryOp::Add { library: p(LIB), folder: p(r"E:\Shared") }]);
    assert!(app.run_action("libraries.new", Instant::now()));
    app.on_paste("Projects");
    key(&mut app, Key::Enter);
    assert_eq!(library_ops(&mut app), [LibraryOp::Create("Projects".into())]);
    assert!(app.run_action("libraries.restore", Instant::now()));
    assert_eq!(library_ops(&mut app), [LibraryOp::RestoreDefaults]);
    app.library_failed("access denied");
    assert!(app.tab().error.as_deref().unwrap().contains("access denied"));
}

#[test]
fn search_inside_a_library_is_limited_to_its_folders() {
    let mut app = library_app();
    app.open_search();
    let fx: Vec<Effect> = app.drain_effects().collect();
    let text = fx.iter().find_map(|e| if let Effect::Search(SearchRequest::Query { text, .. }) = e { Some(text.clone()) } else { None }).unwrap();
    assert!(text.starts_with(r"path:regex:^(?:C\:\\Users\\u\\Documents|D\:\\Docs)\\"), "{text}");
    assert!(fx.iter().any(|e| matches!(e, Effect::Title(t) if t == "Search in Documents — Flit")));
}

#[test]
fn drops_on_a_library_go_to_the_save_folder_and_library_files_open_in_flit() {
    let mut app = library_app();
    let l = app.layout.list;
    let (x, y) = (l.x + l.w / 2.0, l.bottom() - 10.0);
    let paths = vec![p(r"E:\in\a.txt")];
    app.drag(&DragEvent::Enter { paths: paths.clone(), x, y, mods: Mods::default(), allowed: 1 }, Instant::now());
    assert_eq!(app.drag(&DragEvent::Drop { x, y, mods: Mods::default(), allowed: 1 }, Instant::now()), 1);
    let ops: Vec<FileOp> = app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Op { op, .. }, .. } = e { Some(op) } else { None }).collect();
    assert_eq!(ops, [FileOp::Copy { items: paths, dest: p(r"C:\Users\u\Documents") }]);
    app.navigate(Location::Local(p(LIB).parent().unwrap()), Instant::now());
    let t = app.tab();
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    b.push(b"Documents.library-ms", Meta::default());
    let (listing, keys) = (b.listing, b.keys);
    let snap = DirSnapshot { gen: t.gen, path: p(LIB).parent().unwrap(), listing, keys, order: vec![0], view: t.view_spec, done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.open_entry(0, Instant::now());
    assert_eq!(app.current_location(), &Location::Local(p(LIB)), "a library file opens as a location, not in Explorer");
}
