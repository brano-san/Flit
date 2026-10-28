use super::*;
use crate::hash::FxHashMap;
use crate::input::{Event, MouseButton};
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::sort::{CaseMode, SortKeys, ViewSpec};
use crate::tags::{FileKey, TagStore};
use crate::text::tests::Mono;
use crate::ui::{Column, Columns, MenuItem, Persisted, SearchRequest, ShellMenu};

fn tags_with(names: &[&str], files: &[(&str, u32, bool)]) -> Arc<Tags> {
    let mut s = TagStore { tags: Vec::new(), files: FxHashMap::default() };
    for (k, n) in names.iter().enumerate() {
        s.add(n, COLORS[k].1).unwrap();
    }
    let mut seen = FxHashMap::default();
    for (k, &(p, mask, dir)) in files.iter().enumerate() {
        let key = FileKey { volume: 1, id: k as u64 };
        s.update(key, FsPath::parse(p).unwrap(), |_| mask);
        seen.insert(key, (Meta { attrs: if dir { attr::DIR } else { 0 }, ..Meta::default() }, true));
    }
    Arc::new(Tags::new(1, &s, &seen))
}

fn app_with(items: &[(&str, u32)]) -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    app.set_tags(tags_with(&["Important", "Work", "Old"], &[]));
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, mask) in items {
        let i = listing.push(n.as_bytes(), Meta::default());
        listing.set_tags(i, mask);
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    app
}

fn select(app: &mut App, names: &[&str]) {
    let v = app.tab_mut().view.as_mut().unwrap();
    v.clear_selection();
    for n in names {
        let i = v.find(n.as_bytes()).unwrap();
        v.set_selected(i, true);
        v.focus = Some(i);
    }
}

fn resort_now(app: &mut App) {
    let job = app.drain_effects().find_map(|e| if let Effect::Resort { tab, gen, view, snap } = e { Some((tab, gen, view, snap)) } else { None });
    let (tab, gen, view, snap) = job.expect("resort requested");
    let mut order = Vec::new();
    sort::sort(&snap.listing, &snap.keys, view, &mut order);
    app.on_resorted(tab, gen, view, snap.listing.len(), order);
}

fn masks(app: &App) -> Vec<(String, u32)> {
    let v = app.tab().view.as_ref().unwrap();
    v.rows().iter().map(|&i| (String::from_utf8_lossy(v.listing().name(i)).into_owned(), v.listing().entries[i as usize].tags)).collect()
}

fn requests(app: &mut App) -> Vec<TagRequest> {
    app.drain_effects().filter_map(|e| if let Effect::Tags(r) = e { Some(r) } else { None }).collect()
}

fn menu(app: &mut App) -> ShellMenu {
    app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).expect("a menu")
}

fn labels(items: &[MenuItem], out: &mut Vec<String>) {
    for i in items {
        match i {
            MenuItem::Command { label, checked, .. } => out.push(format!("{label}{}", if *checked { " *" } else { "" })),
            MenuItem::Submenu { label, items } => {
                out.push(format!("[{label}]"));
                labels(items, out);
            }
            MenuItem::Header(_) | MenuItem::Separator => {}
        }
    }
}

fn pick(app: &mut App, text: Option<&str>, row: usize) {
    if let Some(t) = text {
        app.palette_insert(t);
    }
    app.run_palette_item(row, Instant::now());
}

fn path(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

#[test]
fn edit_tags_menu_checks_shared_tags_and_toggles_them() {
    let mut app = app_with(&[("a.txt", 0b011), ("b.txt", 0b010), ("c.txt", 0)]);
    select(&mut app, &["a.txt", "b.txt"]);
    app.context_menu();
    let m = menu(&mut app);
    let mut all = Vec::new();
    labels(&m.menu, &mut all);
    let at = all.iter().position(|l| l == "[Edit tags]").unwrap();
    assert_eq!(all[at + 1..at + 6], ["Important", "Work *", "Old", "New tag", "Remove tags"]);
    let k = all.iter().filter(|l| !l.starts_with('[')).position(|l| l == "Work *").unwrap();
    app.on_event(&Event::MenuCommand(k), Instant::now());
    assert_eq!(requests(&mut app), [TagRequest::Set { items: vec![path(r"C:\x\a.txt"), path(r"C:\x\b.txt")], tag: 1, on: false }]);
    app.toggle_tag(2);
    assert_eq!(requests(&mut app), [TagRequest::Set { items: vec![path(r"C:\x\a.txt"), path(r"C:\x\b.txt")], tag: 2, on: true }]);
    app.run_action("tags.remove", Instant::now());
    assert_eq!(requests(&mut app), [TagRequest::Clear(vec![path(r"C:\x\a.txt"), path(r"C:\x\b.txt")])]);
    select(&mut app, &[]);
    app.remove_tags();
    assert!(requests(&mut app).is_empty());
    assert_eq!(app.tab().error.as_deref(), Some("Select the items to tag"));
    assert_eq!(app.selection_tags(), (0, 0));
}

#[test]
fn changes_from_the_store_retag_open_folders_and_wait_for_new_entries() {
    let mut app = app_with(&[("a.txt", 0), ("b.txt", 0b100), ("c.txt", 0b010)]);
    app.run_action("view.sort.tag", Instant::now());
    resort_now(&mut app);
    let now = Instant::now();
    let fresh = tags_with(&["Important", "Work", "Old"], &[(r"C:\x\a.txt", 0b001, false)]);
    let files = vec![(path(r"C:\x\a.txt"), 0b001), (path(r"C:\x\later.txt"), 0b010), (path(r"C:\elsewhere\z.txt"), 0b010)];
    app.on_tags(TagEvent::Changed { tags: fresh.clone(), files, dropped: None }, now);
    assert!(Arc::ptr_eq(app.tags(), &fresh));
    assert_eq!(app.tag_fixes.len(), 1, "only the entry the open folder has not shown yet waits");
    let order: Vec<String> = masks(&app).into_iter().map(|m| m.0).collect();
    assert_eq!(order, ["a.txt", "c.txt", "b.txt"]);
    app.apply_tag_fixes(now + FIX_WINDOW + Duration::from_secs(1));
    assert!(app.tag_fixes.is_empty(), "fixes expire");
    app.on_tags(TagEvent::Changed { tags: tags_with(&["Important", "Old"], &[]), files: Vec::new(), dropped: Some(1) }, now);
    let mut got = masks(&app);
    got.sort();
    assert_eq!(got, [("a.txt".into(), 0b01), ("b.txt".into(), 0b10), ("c.txt".into(), 0)]);
    assert_eq!(app.tag_defs().len(), 2);
    app.on_tags(TagEvent::Status("Nope".into()), now);
    assert_eq!(app.tab().error.as_deref(), Some("Nope"));
}

#[test]
fn palette_flows_create_rename_recolor_and_delete_tags() {
    let mut app = app_with(&[("a.txt", 0)]);
    select(&mut app, &["a.txt"]);
    app.run_action("tags.new", Instant::now());
    pick(&mut app, Some("work"), 0);
    assert_eq!(app.tab().error.as_deref(), Some("A tag with this name already exists"));
    assert!(matches!(&app.nav.palette, Some(p) if matches!(&p.pick, Pick::Tags(TagPick::Name { tag: None, .. }))));
    app.close_palette();
    app.run_action("tags.new", Instant::now());
    pick(&mut app, Some(" Travel "), 0);
    let p = app.nav.palette.as_ref().unwrap();
    assert_eq!(p.titles.len(), COLORS.len());
    pick(&mut app, None, 3);
    assert_eq!(requests(&mut app), [TagRequest::Create { name: "Travel".into(), color: COLORS[3].1, assign: vec![path(r"C:\x\a.txt")] }]);

    app.run_action("tags.manage", Instant::now());
    assert_eq!(app.nav.palette.as_ref().unwrap().titles, ["Important", "Work", "Old", "New tag"]);
    pick(&mut app, None, 1);
    pick(&mut app, None, 0);
    pick(&mut app, Some("Job"), 0);
    assert_eq!(requests(&mut app), [TagRequest::Rename { tag: 1, name: "Job".into() }]);
    app.run_action("tags.manage", Instant::now());
    pick(&mut app, None, 2);
    pick(&mut app, None, 1);
    assert_eq!(app.nav.palette.as_ref().unwrap().titles[0], "Red");
    pick(&mut app, None, 5);
    assert_eq!(requests(&mut app), [TagRequest::Recolor { tag: 2, color: COLORS[5].1 }]);
    app.run_action("tags.manage", Instant::now());
    pick(&mut app, None, 0);
    pick(&mut app, None, 2);
    pick(&mut app, None, 1);
    assert!(requests(&mut app).is_empty(), "Cancel deletes nothing");
    app.run_action("tags.manage", Instant::now());
    pick(&mut app, None, 0);
    pick(&mut app, None, 2);
    assert!(app.nav.palette.as_ref().unwrap().titles[0].contains("\"Important\""));
    pick(&mut app, None, 0);
    assert_eq!(requests(&mut app), [TagRequest::Delete(0)]);
}

#[test]
fn sidebar_tags_search_and_open_their_items() {
    let mut app = app_with(&[("a.txt", 0)]);
    let files = [(r"C:\docs", 0b10, true), (r"C:\docs\x.pdf", 0b10, false), (r"C:\docs\y.pdf", 0b01, false)];
    app.set_tags(tags_with(&["Important", "Long name"], &files));
    let sb = &app.nav.sidebar;
    let first = sb.items.iter().position(|it| it.label == "Important").unwrap();
    assert_eq!((sb.tag_of(first), sb.tag_of(first + 1), sb.tag_of(first + 2)), (Some(0), Some(1), None));
    app.open_side_item(first + 1, false, Instant::now());
    let query = app.drain_effects().find_map(|e| if let Effect::Search(SearchRequest::Query { text, .. }) = e { Some(text) } else { None });
    assert_eq!(query.as_deref(), Some("tag:\"Long name\""));
    assert!(app.search_active());
    app.close_search();
    let r = app.nav.sidebar.rects[first];
    app.on_event(&Event::MouseDown { x: r.x + 5.0, y: r.y + 5.0, button: MouseButton::Right, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x: r.x + 5.0, y: r.y + 5.0, button: MouseButton::Right }, Instant::now());
    let mut all = Vec::new();
    labels(&menu(&mut app).menu, &mut all);
    assert_eq!(all, ["Show items with this tag", "Open all tagged items", "New tag", "Manage tags"]);
    app.open_all_tagged(1, Instant::now());
    let fx: Vec<Effect> = app.drain_effects().collect();
    let opened: Vec<String> = fx.iter().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } = e { Some(path.display().into_owned()) } else { None }).collect();
    assert_eq!(opened, [r"C:\docs\x.pdf"]);
    assert_eq!(app.tabs.len(), 2, "the tagged folder opens in a new tab");
}

#[test]
fn tag_column_is_opt_in_and_shows_names_and_marks() {
    let mut app = app_with(&[("a.txt", 0b011), ("b.txt", 0)]);
    assert!(!app.columns().contains(&Column::Tag));
    app.run_action("view.column.tag", Instant::now());
    assert_eq!(app.columns().last(), Some(&Column::Tag));
    app.set_column_width(Column::Tag, 180.0);
    assert_eq!(app.tab().columns.width(Column::Tag), 180.0);
    assert_eq!(Columns { hidden: 0x81, ..Columns::default() }.sanitized().hidden, 0x80);
    let a = app.tab_mut().view.as_mut().unwrap().find(b"a.txt").unwrap();
    let arena = bumpalo::Bump::new();
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!(app.cell_text(&arena, v.listing(), a, Column::Tag), "Important, Work");
    app.run_action("view.column.tag", Instant::now());
    let mut g = Gfx::new(Mono, 2048);
    app.frame(&mut g, Instant::now());
    let dots = g.dl.instances.len();
    app.tab_mut().view.as_mut().unwrap().set_tags(&[(a, 0)]);
    app.frame(&mut g, Instant::now());
    assert_eq!(g.dl.instances.len() + 2, dots, "one mark per tag");
}

#[test]
fn finished_moves_and_copies_carry_tags() {
    let mut app = app_with(&[("a.txt", 1)]);
    let report = |op| OpReport { op, items: vec![(path(r"C:\x\a.txt"), path(r"D:\y\a.txt"))] };
    let local = ProviderId::Local;
    app.tags_moved(&local, &report(FileOp::Copy { items: Vec::new(), dest: path(r"D:\y") }));
    app.set_tags(tags_with(&["Important"], &[(r"C:\x\a.txt", 1, false)]));
    app.tags_moved(&local, &report(FileOp::Link { items: Vec::new(), dest: path(r"D:\y") }));
    assert!(requests(&mut app).is_empty(), "nothing is tagged or the operation keeps the files");
    app.tags_moved(&local, &report(FileOp::Move { items: Vec::new(), dest: path(r"D:\y") }));
    app.tags_moved(&local, &report(FileOp::Copy { items: Vec::new(), dest: path(r"D:\y") }));
    let pairs = vec![(path(r"C:\x\a.txt"), path(r"D:\y\a.txt"))];
    assert_eq!(requests(&mut app), [TagRequest::Moved { pairs: pairs.clone(), copy: false }, TagRequest::Moved { pairs, copy: true }]);
    let linux = OpReport { op: FileOp::Rename { item: path("/h/a:b"), name: b"c".to_vec() }, items: vec![(path("/h/a:b"), path("/h/c"))] };
    app.tags_moved(&ProviderId::Wsl("U".into()), &linux);
    let pairs = vec![(path("\\\\wsl.localhost\\U\\h\\a\u{F03A}b"), path(r"\\wsl.localhost\U\h\c"))];
    assert_eq!(requests(&mut app), [TagRequest::Moved { pairs, copy: false }], "tags keep Windows paths for WSL files");
}
