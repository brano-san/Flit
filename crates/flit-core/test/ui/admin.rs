use super::super::{App, Effect, Persisted};
use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::provider::{OpId, ProviderEvent, ProviderRequest};
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::ui::{drop_effect, DragEvent, MenuItem};
use std::sync::Arc;
use std::time::Duration;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };

fn t0() -> Instant {
    Instant::now()
}

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn linux_at(path: &str) -> App {
    let state = Persisted { path: Location::parse(path).unwrap(), ..Default::default() };
    let mut app = App::new(state, Vec::new(), 0, false, 1.0, (1000, 600), t0());
    app.caps = crate::caps::Caps { unix_paths: true };
    app.drain_effects().for_each(drop);
    app
}

fn deliver(app: &mut App, items: &[(&str, bool)]) {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Sensitive);
    for &(n, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, size: 1, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
}

fn select(app: &mut App, names: &[&str]) {
    let v = app.tabs[app.cur].view.as_mut().unwrap();
    v.clear_selection();
    for n in names {
        let i = v.find(n.as_bytes()).unwrap();
        v.set_selected(i, true);
        v.focus = Some(i);
    }
}

fn key(app: &mut App, key: Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, t0());
}

fn requests(app: &mut App) -> Vec<(ProviderId, ProviderRequest)> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { provider, req } = e { Some((provider, req)) } else { None }).collect()
}

fn listed(reqs: &[(ProviderId, ProviderRequest)]) -> Vec<(ProviderId, FsPath)> {
    reqs.iter().filter_map(|(provider, r)| if let ProviderRequest::List { path, .. } = r { Some((provider.clone(), path.clone())) } else { None }).collect()
}

fn sent(app: &mut App) -> Vec<(ProviderId, OpId, FileOp)> {
    requests(app).into_iter().filter_map(|(provider, r)| if let ProviderRequest::Op { id, op, .. } = r { Some((provider, id, op)) } else { None }).collect()
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, .. } => vec![label.clone()],
            MenuItem::Submenu { label, items } => std::iter::once(label.clone()).chain(labels(items)).collect(),
            MenuItem::Header(_) | MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn open_root_tab(app: &mut App, folder: &str) {
    deliver(app, &[(folder, true), ("note.txt", false)]);
    select(app, &[folder]);
    assert!(app.run_action("folder.open_admin", t0()));
}

#[test]
fn open_as_administrator_opens_a_marked_tab_listed_by_the_root_agent() {
    let mut app = linux_at("/home/u");
    open_root_tab(&mut app, "etc");
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(app.current_location(), &Location::Admin(p("/home/u/etc")));
    let reqs = requests(&mut app);
    assert_eq!(listed(&reqs), [(ProviderId::Admin, p("/home/u/etc"))]);
    assert!(reqs.iter().any(|(provider, r)| *provider == ProviderId::Admin && matches!(r, ProviderRequest::Watch { path: Some(_), .. })));
    assert!(app.window_title().contains("(Administrator)"), "{}", app.window_title());
    assert_eq!(super::super::tabbar::tab_glyph(app.current_location()), G_ADMIN);
    let mut g = crate::text::Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    let crumbs = app.layout.crumbs.iter().find(|r| r.w > 0.0).copied().unwrap();
    let badge = g.measure(S::AddressAdminBadge.t(), FontStyle::new(crate::text::FontFamily::Ui, 13.0, false));
    assert!(crumbs.x >= app.layout.address.x + badge, "the badge sits before the path: {crumbs:?} {:?}", app.layout.address);

    app.select_tab(0);
    app.frame(&mut g, t0());
    let plain = app.layout.crumbs.iter().find(|r| r.w > 0.0).copied().unwrap();
    assert!(plain.x < app.layout.address.x + badge, "an ordinary tab has no badge");
    assert!(!app.window_title().contains("Administrator"));
}

#[test]
fn without_a_selected_folder_the_command_opens_the_current_folder_as_root() {
    let mut app = linux_at("/srv/data");
    deliver(&mut app, &[("note.txt", false)]);
    select(&mut app, &["note.txt"]);
    assert!(app.run_action("folder.open_admin", t0()));
    assert_eq!(app.current_location(), &Location::Admin(p("/srv/data")));
    assert_eq!(listed(&requests(&mut app)), [(ProviderId::Admin, p("/srv/data"))]);
}

#[test]
fn navigation_inside_a_root_tab_stays_with_the_root_agent() {
    let mut app = linux_at("/home/u");
    open_root_tab(&mut app, "etc");
    requests(&mut app);
    deliver(&mut app, &[("ssh", true)]);
    select(&mut app, &["ssh"]);
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.current_location(), &Location::Admin(p("/home/u/etc/ssh")));
    assert_eq!(listed(&requests(&mut app)), [(ProviderId::Admin, p("/home/u/etc/ssh"))]);
    app.navigate(Location::parse("/var/log").unwrap(), t0());
    assert_eq!(app.current_location(), &Location::Admin(p("/var/log")), "a typed path stays in the root tab");
    key(&mut app, Key::Up, Mods { ctrl: false, shift: false, alt: true });
    assert_eq!(app.current_location(), &Location::Admin(p("/var")));
}

#[test]
fn file_operations_in_a_root_tab_run_in_the_root_agent_and_delete_permanently() {
    let mut app = linux_at("/home/u");
    open_root_tab(&mut app, "etc");
    requests(&mut app);
    deliver(&mut app, &[("hosts", false), ("ssh", true)]);
    select(&mut app, &["hosts"]);
    key(&mut app, Key::F(2), NO);
    for c in "hosts2".chars() {
        app.on_event(&Event::Char(c), t0());
    }
    key(&mut app, Key::Enter, NO);
    let renamed = sent(&mut app);
    assert!(matches!(&renamed[..], [(ProviderId::Admin, _, FileOp::Rename { item, .. })] if *item == p("/home/u/etc/hosts")), "{renamed:?}");

    assert!(app.run_action("file.new_folder", t0()));
    let made = sent(&mut app);
    assert!(matches!(&made[..], [(ProviderId::Admin, _, FileOp::NewFolder { dir, .. })] if *dir == p("/home/u/etc")), "{made:?}");

    assert!(app.run_action("file.paste", t0()));
    assert_eq!(sent(&mut app).into_iter().map(|(pr, _, op)| (pr, op)).collect::<Vec<_>>(), [(ProviderId::Admin, FileOp::Paste { dest: p("/home/u/etc") })]);

    select(&mut app, &["ssh"]);
    key(&mut app, Key::Delete, NO);
    assert_eq!(app.asking(), Some("confirm"), "root has no trash, so Delete asks to delete permanently");
    key(&mut app, Key::Enter, NO);
    assert_eq!(sent(&mut app).into_iter().map(|(pr, _, op)| (pr, op)).collect::<Vec<_>>(), [(ProviderId::Admin, FileOp::Delete(vec![p("/home/u/etc/ssh")]))]);
}

#[test]
fn ordinary_tabs_never_go_through_the_root_agent() {
    let mut app = linux_at("/home/u");
    open_root_tab(&mut app, "etc");
    requests(&mut app);
    app.select_tab(0);
    requests(&mut app);
    deliver(&mut app, &[("etc", true), ("note.txt", false)]);
    select(&mut app, &["note.txt"]);
    key(&mut app, Key::Delete, NO);
    assert!(app.run_action("file.new_folder", t0()));
    assert!(app.run_action("file.paste", t0()));
    let ops = sent(&mut app);
    assert_eq!(ops.len(), 3, "{ops:?}");
    assert!(ops.iter().all(|(provider, _, _)| *provider == ProviderId::Local), "{ops:?}");
    assert!(matches!(&ops[0].2, FileOp::Trash(_)), "ordinary tabs keep the trash");
    app.navigate(Location::parse("/var").unwrap(), t0());
    assert_eq!(listed(&requests(&mut app)), [(ProviderId::Local, p("/var"))]);
}

#[test]
fn dragging_from_a_root_tab_into_an_ordinary_tab_runs_through_the_root_agent() {
    use drop_effect::*;
    let mut app = linux_at("/home/u");
    open_root_tab(&mut app, "etc");
    requests(&mut app);
    deliver(&mut app, &[("hosts", false), ("passwd", false)]);
    select(&mut app, &["hosts"]);
    let mut g = crate::text::Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    let row = |app: &App, k: usize| (app.layout.cols[0].x + 100.0, app.layout.list.y + (k as f32 + 0.5) * app.layout.row_h);
    let (x, y) = row(&app, 0);
    app.on_event(&Event::Key { key: Key::Other, mods: NO, pressed: false }, t0());
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t0());
    app.on_event(&Event::MouseMove { x: x + 12.0, y: y + 3.0 }, t0());
    let dragged = app.drain_effects().find_map(|e| if let Effect::StartDrag(paths) = e { Some(paths) } else { None }).expect("a drag starts");
    assert_eq!(dragged, [p("/home/u/etc/hosts")]);
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, t0());

    app.select_tab(0);
    requests(&mut app);
    deliver(&mut app, &[("note.txt", false)]);
    app.frame(&mut g, t0());
    let empty = (300.0, app.layout.list.y + 200.0);
    let all = COPY | MOVE | LINK;
    app.drag(&DragEvent::Enter { paths: dragged.clone(), x: empty.0, y: empty.1, mods: NO, allowed: all, right: false }, t0());
    app.drag(&DragEvent::Drop { x: empty.0, y: empty.1, mods: NO, allowed: all }, t0());
    let ops = sent(&mut app);
    assert!(matches!(&ops[..], [(ProviderId::Admin, _, FileOp::Copy { items, dest } | FileOp::Move { items, dest })] if *items == dragged && *dest == p("/home/u")), "{ops:?}");

    let outside = vec![p("/tmp/other.txt")];
    app.drag(&DragEvent::Enter { paths: outside, x: empty.0, y: empty.1, mods: NO, allowed: all, right: false }, t0());
    app.drag(&DragEvent::Drop { x: empty.0, y: empty.1, mods: NO, allowed: all }, t0());
    let ops = sent(&mut app);
    assert!(matches!(&ops[..], [(ProviderId::Local, _, _)]), "a drag from elsewhere stays ordinary: {ops:?}");
    app.drag(&DragEvent::Enter { paths: dragged.clone(), x: empty.0, y: empty.1, mods: NO, allowed: all, right: false }, t0());
    app.drag(&DragEvent::Drop { x: empty.0, y: empty.1, mods: NO, allowed: all }, t0());
    assert!(matches!(&sent(&mut app)[..], [(ProviderId::Local, _, _)]), "a later outside drag forgets the root drag");
}

#[test]
fn a_refused_password_closes_the_root_tab_and_says_why() {
    let mut app = linux_at("/home/u");
    open_root_tab(&mut app, "etc");
    requests(&mut app);
    let tab = app.tab().id;
    let error = crate::tr_en!(S::ErrorsEngineAdminDenied, why = S::ErrorsOpsExtraAdminDismissed.en());
    app.on_provider_event(ProviderEvent::Denied { tab, error }, t0());
    assert_eq!(app.tabs().len(), 1, "no empty root tab is left behind");
    assert_eq!(app.current_location(), &Location::parse("/home/u").unwrap());
    assert_eq!(app.tab().error.as_deref(), Some("Cannot open the folder as administrator: the password prompt was dismissed"));
    assert!(app.nav.closed.is_empty(), "the refused tab is not offered for reopening");
    open_root_tab(&mut app, "etc");
    assert_eq!(listed(&requests(&mut app)), [(ProviderId::Admin, p("/home/u/etc"))], "opening again asks the root agent, and so the password, again");
}

#[test]
fn a_refused_only_tab_falls_back_to_the_same_folder_without_root() {
    let mut app = linux_at("/home/u");
    open_root_tab(&mut app, "etc");
    app.select_tab(0);
    let first = app.tab().id;
    app.close_tab(0);
    assert_ne!(app.tab().id, first);
    requests(&mut app);
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::Denied { tab, error: S::ErrorsEngineAdminSilent.en().into() }, t0());
    assert_eq!(app.tabs().len(), 1);
    assert_eq!(app.current_location(), &Location::parse("/home/u/etc").unwrap());
    assert_eq!(listed(&requests(&mut app)), [(ProviderId::Local, p("/home/u/etc"))]);
}

#[test]
fn a_crashed_agent_leaves_the_tab_with_a_message_and_f5_asks_again() {
    let mut app = linux_at("/home/u");
    open_root_tab(&mut app, "etc");
    requests(&mut app);
    deliver(&mut app, &[("hosts", false)]);
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::OpDone { tab, id: 0, error: Some(S::ErrorsEngineAdminStopped.en().into()), report: None }, t0());
    assert_eq!(app.tabs().len(), 2, "a root tab that worked stays open");
    assert!(app.tab().error.as_deref().is_some_and(|e| e.contains("Press F5")));
    key(&mut app, Key::F(5), NO);
    assert_eq!(listed(&requests(&mut app)), [(ProviderId::Admin, p("/home/u/etc"))]);
}

#[test]
fn windows_does_not_offer_open_as_administrator() {
    let state = Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() };
    let mut app = App::new(state, Vec::new(), 0, false, 1.0, (1000, 600), t0());
    assert!(!crate::caps::Caps { unix_paths: false }.supports("folder.open_admin"));
    assert!(crate::caps::Caps { unix_paths: true }.supports("folder.open_admin"));
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    let i = listing.push(b"sub", Meta { attrs: attr::DIR, ..Default::default() });
    keys.push(listing.name(i));
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order: vec![0], view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    select(&mut app, &["sub"]);
    let label = S::ActionsFolderOpenAdmin.t().to_owned();
    assert!(!labels(&app.build_menu(true)).contains(&label));
    assert!(!labels(&app.build_menu(false)).contains(&label));
    assert!(!app.run_action("folder.open_admin", t0()));
    assert_eq!(app.tabs().len(), 1);

    let mut linux = linux_at("/home/u");
    deliver(&mut linux, &[("sub", true)]);
    select(&mut linux, &["sub"]);
    assert!(labels(&linux.build_menu(true)).contains(&label));
    assert!(labels(&linux.build_menu(false)).contains(&label));
}

#[test]
fn a_saved_root_tab_comes_back_as_an_ordinary_tab() {
    let root = Location::Admin(p("/etc"));
    assert_eq!(Location::parse_exact(&root.to_bytes()), Some(Location::Local(p("/etc"))));
    assert_eq!(admin_location(&Location::Local(p("/a/b.zip"))), None, "archives are not opened as root");
    assert_eq!(admin_location(&Location::Wsl { distro: "Ubuntu".into(), path: p("/etc") }), None);
}
