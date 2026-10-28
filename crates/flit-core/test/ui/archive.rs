use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::provider::{Conflict, ProviderEvent, Resolve};
use crate::render::Rect;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::ui::{drop_effect, DragEvent, Persisted};
use std::sync::{mpsc, Arc};
use std::time::Duration;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const FILE: u32 = 0;
const DIR: u32 = attr::DIR;
const PACKED: u32 = attr::ARCHIVED | attr::SIZE_KNOWN;

fn t0() -> Instant {
    Instant::now()
}

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn system_app_at(path: &str) -> App {
    App::new(Persisted { path: Location::parse(path).unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), t0())
}

fn app_at(path: &str) -> App {
    let mut app = system_app_at(path);
    app.builtin_archives = true;
    app
}

fn deliver(app: &mut App, items: &[(&str, u32)]) {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, attrs) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs, size: 10, ..Default::default() });
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

fn typed(app: &mut App, s: &str) {
    s.chars().for_each(|c| app.on_event(&Event::Char(c), t0()));
}

fn ops(app: &mut App) -> Vec<(OpId, FileOp)> {
    app.drain_effects()
        .filter_map(|e| match e {
            Effect::Provider { req: ProviderRequest::Op { id, op, .. }, .. } => Some((id, op)),
            _ => None,
        })
        .collect()
}

fn only(app: &mut App) -> (OpId, FileOp) {
    let mut v = ops(app);
    assert_eq!(v.len(), 1, "{v:?}");
    v.remove(0)
}

fn form(app: &App) -> &Form {
    app.jobs.form.as_ref().unwrap()
}

fn finish(app: &mut App, id: OpId, op: FileOp, items: &[(&str, &str)], error: Option<&str>) {
    let tab = app.tab().id;
    let items = items.iter().map(|(a, b)| (p(a), p(b))).collect();
    app.on_provider_event(ProviderEvent::OpDone { tab, id, error: error.map(str::to_owned), report: Some(OpReport { op, items }) }, t0());
}

#[test]
fn the_compress_dialog_starts_a_background_job_and_remembers_its_choices_without_the_password() {
    let mut app = app_at(r"C:\cmp");
    deliver(&mut app, &[("docs", DIR), ("a.txt", FILE)]);
    select(&mut app, &["docs"]);
    assert!(app.run_action("archive.compress", t0()));
    assert_eq!(app.asking(), Some("compress"));
    assert_eq!(form(&app).text(C_NAME), "docs.zip");
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Right, NO);
    assert_eq!(form(&app).text(C_NAME), "docs.7z");
    assert_eq!(form(&app).choice(C_LEVEL), Format::SevenZ.default_level() as usize);
    for _ in C_FORMAT..C_PASSWORD {
        key(&mut app, Key::Down, NO);
    }
    typed(&mut app, "pw");
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Space, NO);
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.asking(), None);
    let (_, FileOp::Archive(ArchiveOp::Compress { items, archive, options })) = only(&mut app) else { panic!("no compress op") };
    assert_eq!((items, archive), (vec![p(r"C:\cmp\docs")], p(r"C:\cmp\docs.7z")));
    assert_eq!((options.format, options.password, options.encrypt_names), (Format::SevenZ, Some(Secret("pw".into())), true));
    let kept = app.persisted().archive.compression;
    assert_eq!((kept.format, kept.level, kept.password), (Format::SevenZ, 5, None));
}

#[test]
fn a_password_for_tar_is_refused_inside_the_dialog() {
    let mut app = app_at(r"C:\cmp_tar");
    deliver(&mut app, &[("a.txt", FILE)]);
    select(&mut app, &["a.txt"]);
    app.compress_dialog();
    let f = app.jobs.form.as_mut().unwrap();
    if let Some(Control::Choice { sel, .. }) = f.controls.get_mut(C_FORMAT) {
        *sel = 2;
    }
    app.form_changed(C_FORMAT);
    app.jobs.form.as_mut().unwrap().set_text(C_PASSWORD, "pw");
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.asking(), Some("compress"));
    assert!(form(&app).error && ops(&mut app).is_empty());
    assert_eq!(form(&app).text(C_NAME), "a.tar");
}

#[test]
fn quick_compress_names_the_archive_after_the_item_and_skips_taken_names() {
    let mut app = app_at(r"C:\quick");
    deliver(&mut app, &[("docs", DIR), ("docs.zip", FILE)]);
    select(&mut app, &["docs"]);
    app.run_action("archive.zip", t0());
    let (_, FileOp::Archive(ArchiveOp::Compress { archive, options, .. })) = only(&mut app) else { panic!("no compress op") };
    assert_eq!((archive, options.format), (p(r"C:\quick\docs (2).zip"), Format::Zip));
    app.run_action("archive.7z", t0());
    let (_, FileOp::Archive(ArchiveOp::Compress { archive, .. })) = only(&mut app) else { panic!("no compress op") };
    assert_eq!(archive, p(r"C:\quick\docs.7z"));
}

#[test]
fn extraction_keys_dialog_history_and_opening_the_folder_when_done() {
    let mut app = app_at(r"C:\ext");
    deliver(&mut app, &[("a.zip", FILE), ("b.7z", FILE), ("c.txt", FILE)]);
    select(&mut app, &["a.zip"]);
    app.run_action("archive.smart", Instant::now());
    let smart = FileOp::Archive(ArchiveOp::Extract { archives: vec![p(r"C:\ext\a.zip")], dest: None, place: Place::Smart });
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [smart]);
    key(&mut app, Key::Char('E'), Mods { alt: true, ..NO });
    assert_eq!(app.asking(), Some("extract"));
    assert_eq!(form(&app).text(E_DEST), r"C:\ext\a");
    for _ in 0..E_OPEN {
        key(&mut app, Key::Down, NO);
    }
    key(&mut app, Key::Space, NO);
    key(&mut app, Key::Enter, NO);
    let (id, op) = only(&mut app);
    assert_eq!(op, FileOp::Archive(ArchiveOp::Extract { archives: vec![p(r"C:\ext\a.zip")], dest: Some(p(r"C:\ext")), place: Place::OwnFolder }));
    assert_eq!(app.persisted().archive.history, [r"C:\ext\a"]);
    finish(&mut app, id, op, &[(r"C:\ext\a.zip", r"C:\ext\a")], None);
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(app.current_location(), &Location::parse(r"C:\ext\a").unwrap());

    app.select_tab(0);
    select(&mut app, &["a.zip", "b.7z"]);
    app.extract_dialog();
    assert_eq!(form(&app).text(E_DEST), r"C:\ext");
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Right, NO);
    app.jobs.form.as_mut().unwrap().set_text(E_DEST, r"D:\out");
    key(&mut app, Key::Enter, NO);
    let (_, op) = only(&mut app);
    assert_eq!(op, FileOp::Archive(ArchiveOp::Extract { archives: vec![p(r"C:\ext\a.zip"), p(r"C:\ext\b.7z")], dest: Some(p(r"D:\out")), place: Place::OwnFolder }));
    assert_eq!(app.persisted().archive.history, [r"D:\out", r"C:\ext\a"]);
}

#[test]
fn an_encrypted_archive_asks_for_its_password_and_retries_only_what_is_left() {
    let mut app = app_at(r"C:\locked");
    deliver(&mut app, &[("a.zip", FILE), ("b.zip", FILE)]);
    select(&mut app, &["a.zip", "b.zip"]);
    app.run_action("archive.extract_here", t0());
    let (id, op) = only(&mut app);
    finish(&mut app, id, op, &[(r"C:\locked\a.zip", r"C:\locked\a.txt")], Some(crate::i18n::S::ErrorsArchivePasswordNeeded.en()));
    assert_eq!(app.asking(), Some("password"));
    assert_eq!(form(&app).controls.len(), 2);
    typed(&mut app, "secret");
    key(&mut app, Key::Enter, NO);
    assert_eq!(archive::password(&p(r"C:\locked\b.zip")), Some(Secret("secret".into())));
    assert_eq!(archive::password(&p(r"C:\locked\a.zip")), None);
    let (id, op) = only(&mut app);
    assert_eq!(op, FileOp::Archive(ArchiveOp::Extract { archives: vec![p(r"C:\locked\b.zip")], dest: None, place: Place::Here }));
    finish(&mut app, id, op, &[], Some(crate::i18n::S::ErrorsArchiveWrongPassword.en()));
    assert_eq!(app.asking(), Some("password"));
    assert!(form(&app).error);
    key(&mut app, Key::Escape, NO);
    assert_eq!(app.asking(), None);
    assert!(ops(&mut app).is_empty());
}

#[test]
fn archives_open_as_folders_and_files_inside_open_from_a_temporary_copy() {
    let mut app = app_at(r"C:\browse");
    app.archives_as_folders = true;
    deliver(&mut app, &[("a.zip", FILE), ("report.docx", FILE)]);
    select(&mut app, &["report.docx"]);
    key(&mut app, Key::Enter, NO);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { .. }, .. })));
    select(&mut app, &["a.zip"]);
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.current_location(), &Location::parse(r"C:\browse\a.zip").unwrap());
    deliver(&mut app, &[("docs", DIR | PACKED), ("readme.txt", PACKED), ("inner.zip", PACKED)]);
    assert_eq!(app.archive_view(), Some((p(r"C:\browse\a.zip"), Vec::new())));
    assert_eq!(app.unpacked_total(), Some(30));
    select(&mut app, &["readme.txt"]);
    key(&mut app, Key::Enter, NO);
    let (id, op) = only(&mut app);
    assert_eq!(op, FileOp::Archive(ArchiveOp::Open { item: p(r"C:\browse\a.zip\readme.txt") }));
    finish(&mut app, id, op, &[(r"C:\browse\a.zip\readme.txt", r"C:\tmp\1\readme.txt")], None);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path == p(r"C:\tmp\1\readme.txt"))));
    select(&mut app, &["inner.zip"]);
    key(&mut app, Key::Enter, NO);
    let (id, op) = only(&mut app);
    finish(&mut app, id, op, &[(r"C:\browse\a.zip\inner.zip", r"C:\tmp\2\inner.zip")], None);
    assert_eq!(app.current_location(), &Location::parse(r"C:\tmp\2\inner.zip").unwrap());
}

#[test]
fn by_default_archives_open_in_the_system_app_and_open_as_folder_still_browses_them() {
    let mut app = app_at(r"C:\browse");
    assert!(!app.archives_as_folders);
    deliver(&mut app, &[("a.zip", FILE), ("b.7z", FILE)]);
    select(&mut app, &["a.zip"]);
    key(&mut app, Key::Enter, NO);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path == p(r"C:\browse\a.zip"))));
    assert_eq!(app.current_location(), &Location::parse(r"C:\browse").unwrap());
    select(&mut app, &["a.zip", "b.7z"]);
    key(&mut app, Key::Enter, NO);
    let opened: Vec<FsPath> = app
        .drain_effects()
        .filter_map(|e| match e {
            Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } => Some(path),
            _ => None,
        })
        .collect();
    assert_eq!(opened, [p(r"C:\browse\a.zip"), p(r"C:\browse\b.7z")]);
    assert_eq!(app.tabs().len(), 1);
    select(&mut app, &["a.zip"]);
    app.open_as_folder(t0());
    assert_eq!(app.current_location(), &Location::parse(r"C:\browse\a.zip").unwrap());
    deliver(&mut app, &[("inner.zip", PACKED)]);
    select(&mut app, &["inner.zip"]);
    key(&mut app, Key::Enter, NO);
    let (id, op) = only(&mut app);
    finish(&mut app, id, op, &[(r"C:\browse\a.zip\inner.zip", r"C:\tmp\2\inner.zip")], None);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path == p(r"C:\tmp\2\inner.zip"))));
    assert_eq!(app.current_location(), &Location::parse(r"C:\browse\a.zip").unwrap());
}

#[test]
fn the_archives_as_folders_setting_is_read_from_the_settings_file() {
    let mut app = app_at(r"C:\browse");
    let i = crate::ui::settings::index("files.archives_as_folders").unwrap();
    assert_eq!(crate::ui::settings::defaults()[i], crate::ui::settings::Value::Bool(false));
    app.apply_doc(&crate::ui::settings::parse(r#"{"files.archives_as_folders": true}"#));
    assert!(app.archives_as_folders);
    deliver(&mut app, &[("a.zip", FILE)]);
    select(&mut app, &["a.zip"]);
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.current_location(), &Location::parse(r"C:\browse\a.zip").unwrap());
}

fn labels_deep(items: &[crate::ui::MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            crate::ui::MenuItem::Command { label, .. } => vec![label.clone()],
            crate::ui::MenuItem::Submenu { label, items } => std::iter::once(label.clone()).chain(labels_deep(items)).collect(),
            crate::ui::MenuItem::Header(_) | crate::ui::MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn settings_page_lines(app: &mut App, page: crate::ui::settings::Page) -> Vec<String> {
    app.open_settings(Some(page));
    let lines = app.settings_lines();
    app.close_settings();
    lines
}

#[test]
fn with_builtin_handling_off_archives_go_to_the_system_app_and_flits_archive_commands_and_settings_are_inactive() {
    use crate::ui::settings::{defaults, index, Page, Value, BUILTIN_ARCHIVES, NEEDS_BUILTIN_ARCHIVES};
    assert_eq!(defaults()[index(BUILTIN_ARCHIVES).unwrap()], Value::Bool(false));
    let mut app = system_app_at(r"C:\sys");
    app.archives_as_folders = true;
    deliver(&mut app, &[("a.zip", FILE), ("b.txt", FILE)]);
    select(&mut app, &["a.zip"]);
    key(&mut app, Key::Enter, NO);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path == p(r"C:\sys\a.zip"))));
    assert_eq!(app.current_location(), &Location::parse(r"C:\sys").unwrap());
    let menu = labels_deep(&app.build_menu(true));
    for gone in ["Compress", "Extract", "Add to archive…", "Extract here", "Open as folder"] {
        assert!(!menu.iter().any(|l| l == gone), "{gone} in {menu:?}");
    }
    assert!(menu.contains(&"Copy".to_owned()) && menu.contains(&"Properties".to_owned()));
    select(&mut app, &["b.txt"]);
    assert!(labels_deep(&app.build_menu(true)).contains(&"Hashes…".to_owned()), "Hashes stays without the archive commands");
    select(&mut app, &["a.zip"]);
    key(&mut app, Key::Char('E'), Mods { alt: true, ..NO });
    assert!(app.jobs.form.is_none());
    for id in ["archive.compress", "archive.zip", "archive.extract", "archive.extract_here", "archive.open"] {
        assert!(!app.run_action(id, t0()), "{id}");
        assert!(!app.available(id), "{id}");
    }
    assert!(ops(&mut app).is_empty());
    let mut lines = settings_page_lines(&mut app, Page::Files);
    lines.extend(settings_page_lines(&mut app, Page::General));
    assert!(lines.contains(&BUILTIN_ARCHIVES.to_owned()), "{lines:?}");
    for id in NEEDS_BUILTIN_ARCHIVES {
        assert!(lines.contains(&format!("off {id}")) && !lines.contains(&id.to_owned()), "{id} in {lines:?}");
    }
    app.builtin_archives = true;
    let mut lines = settings_page_lines(&mut app, Page::Files);
    lines.extend(settings_page_lines(&mut app, Page::General));
    for id in NEEDS_BUILTIN_ARCHIVES {
        assert!(lines.contains(&id.to_owned()), "{id} in {lines:?}");
    }
    let menu = labels_deep(&app.build_menu(true));
    assert!(menu.contains(&"Compress".to_owned()) && menu.contains(&"Extract".to_owned()), "{menu:?}");
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.current_location(), &Location::parse(r"C:\sys\a.zip").unwrap());
}

#[test]
fn settings_files_that_browsed_archives_keep_builtin_handling_on() {
    let mut app = system_app_at(r"C:\browse");
    app.apply_doc(&crate::ui::settings::parse(r#"{"schema": 1, "files.archives_as_folders": true}"#));
    assert!(app.builtin_archives && app.archives_as_folders);
    let mut off = system_app_at(r"C:\browse");
    off.apply_doc(&crate::ui::settings::parse(r#"{"schema": 1, "files.archives_as_folders": false}"#));
    assert!(!off.builtin_archives);
    let mut chosen = system_app_at(r"C:\browse");
    chosen.apply_doc(&crate::ui::settings::parse(r#"{"schema": 2, "files.archives_as_folders": true}"#));
    assert!(!chosen.builtin_archives && chosen.archives_as_folders, "a current file keeps the choice it records");
    deliver(&mut app, &[("a.zip", FILE)]);
    select(&mut app, &["a.zip"]);
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.current_location(), &Location::parse(r"C:\browse\a.zip").unwrap());
}

#[test]
fn archives_open_like_folders_in_new_tabs_when_asked() {
    let mut app = app_at(r"C:\browse");
    app.archives_as_folders = true;
    app.folders_in_new_tab = true;
    deliver(&mut app, &[("a.zip", FILE), ("b.7z", FILE)]);
    select(&mut app, &["a.zip"]);
    key(&mut app, Key::Enter, NO);
    assert_eq!((app.tabs().len(), app.current_location()), (2, &Location::parse(r"C:\browse\a.zip").unwrap()));
    let mut app = app_at(r"C:\browse");
    app.archives_as_folders = true;
    deliver(&mut app, &[("a.zip", FILE), ("b.7z", FILE)]);
    select(&mut app, &["a.zip", "b.7z"]);
    key(&mut app, Key::Enter, NO);
    let opened: Vec<_> = app.tabs().iter().map(|t| t.location().clone()).collect();
    assert_eq!(opened[1..], [Location::parse(r"C:\browse\a.zip").unwrap(), Location::parse(r"C:\browse\b.7z").unwrap()]);
}

#[test]
fn the_preview_pane_asks_for_nothing_inside_an_archive() {
    let mut app = app_at(r"C:\pv\a.zip");
    app.set_pane_state(crate::ui::PaneState { visible: true, ..Default::default() });
    deliver(&mut app, &[("readme.txt", PACKED), ("docs", DIR | PACKED)]);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    for name in ["readme.txt", "docs"] {
        select(&mut app, &[name]);
        app.frame(&mut g, t0());
        assert!(!app.drain_effects().any(|e| matches!(e, Effect::Preview(_))));
    }
}

#[test]
fn inside_an_archive_cut_and_rename_are_refused_and_the_menu_is_flits_own() {
    let mut app = app_at(r"C:\menu\a.zip\docs");
    deliver(&mut app, &[("x.txt", PACKED)]);
    select(&mut app, &["x.txt"]);
    app.cut_selection();
    assert_eq!(app.tab().error.as_deref(), Some(crate::i18n::S::ErrorsArchiveInside.en()));
    app.rename_focused();
    assert!(!app.is_renaming());
    let labels = |items: &[crate::ui::MenuItem]| items.iter().filter_map(|i| if let crate::ui::MenuItem::Command { label, .. } = i { Some(label.clone()) } else { None }).collect::<Vec<_>>();
    let on_items = labels(&app.build_menu(true));
    assert!(on_items.contains(&"Extract…".to_owned()) && on_items.contains(&"Copy".to_owned()));
    assert!(!on_items.iter().any(|l| l == "Cut" || l == "Rename" || l == "Delete" || l == "Properties"));
    let on_folder = labels(&app.build_menu(false));
    assert!(on_folder.contains(&"Encoding of names…".to_owned()) && on_folder.contains(&"Paste".to_owned()));
}

#[test]
fn the_context_menu_offers_extract_and_compress_by_selection() {
    let mut app = app_at(r"C:\ctx");
    deliver(&mut app, &[("a.zip", FILE), ("b.txt", FILE)]);
    select(&mut app, &["a.zip"]);
    let menu = app.build_menu(true);
    let sub = |label: &str| {
        menu.iter().find_map(|i| match i {
            crate::ui::MenuItem::Submenu { label: l, items } if l == label => Some(items.iter().filter_map(|i| if let crate::ui::MenuItem::Command { label, .. } = i { Some(label.clone()) } else { None }).collect::<Vec<_>>()),
            _ => None,
        })
    };
    assert_eq!(sub("Extract").unwrap(), ["Extract files…", "Extract here", "Smart extract", "Extract to \u{201C}a\\\u{201D}", "Open as folder"]);
    assert_eq!(sub("Compress").unwrap(), ["Add to archive…", "Compress to \u{201C}a (2).zip\u{201D}", "Compress to \u{201C}a.7z\u{201D}"]);
    select(&mut app, &["b.txt"]);
    let menu = app.build_menu(true);
    assert!(!menu.iter().any(|i| matches!(i, crate::ui::MenuItem::Submenu { label, .. } if label == "Extract")));
    assert!(menu.iter().any(|i| matches!(i, crate::ui::MenuItem::Command { label, .. } if label == "Hashes…")));
}

#[test]
fn the_status_bar_picks_the_encoding_of_zip_names_and_reloads_the_view() {
    let mut app = app_at(r"C:\enc\old.zip");
    deliver(&mut app, &[("readme.txt", PACKED)]);
    assert_eq!(app.encoding_label(), "Names: Automatic");
    app.pick_encoding();
    assert!(app.nav.palette.as_ref().is_some_and(|pal| pal.pick == Pick::Encoding(p(r"C:\enc\old.zip"))));
    let k = 1 + Encoding::ALL.iter().position(|&e| e == Encoding::Cp866).unwrap();
    let row = app.nav.palette.as_ref().unwrap().matches.iter().position(|&m| m == k).unwrap();
    app.run_palette_item(row, t0());
    assert_eq!(archive::chosen_encoding(&p(r"C:\enc\old.zip")), Some(Encoding::Cp866));
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. }, .. })));
    assert_eq!(app.encoding_label(), "Names: CP866");
    let mut seven = app_at(r"C:\enc\new.7z");
    deliver(&mut seven, &[("readme.txt", PACKED)]);
    assert_eq!(seven.encoding_label(), "");
}

#[test]
fn files_dropped_on_a_zip_are_copied_into_it_never_moved() {
    let mut app = app_at(r"C:\drop");
    deliver(&mut app, &[("a.zip", FILE), ("b.7z", FILE)]);
    let row = |app: &App, n: usize| (app.layout.cols[0].x + 100.0, app.layout.list.y + (n as f32 + 0.5) * app.layout.row_h);
    let all = drop_effect::COPY | drop_effect::MOVE;
    let at = row(&app, 0);
    let enter = DragEvent::Enter { paths: vec![p(r"C:\drop\f.txt")], x: at.0, y: at.1, mods: Mods { shift: true, ..NO }, allowed: all };
    assert_eq!(app.drag(&enter, t0()), drop_effect::COPY);
    app.drag(&DragEvent::Drop { x: at.0, y: at.1, mods: Mods { shift: true, ..NO }, allowed: all }, t0());
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Copy { items: vec![p(r"C:\drop\f.txt")], dest: p(r"C:\drop\a.zip") }]);
    let at = row(&app, 1);
    let enter = DragEvent::Enter { paths: vec![p(r"D:\f.txt")], x: at.0, y: at.1, mods: NO, allowed: all };
    assert_eq!(app.drag(&enter, t0()), drop_effect::COPY);
    app.drag(&DragEvent::Drop { x: at.0, y: at.1, mods: NO, allowed: all }, t0());
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), [FileOp::Copy { items: vec![p(r"D:\f.txt")], dest: p(r"C:\drop") }]);
}

#[test]
fn unreadable_items_are_skipped_or_cancel_the_compression() {
    let mut app = app_at(r"C:\unread");
    deliver(&mut app, &[("a.txt", FILE)]);
    select(&mut app, &["a.txt"]);
    app.run_action("archive.zip", t0());
    let (id, ctl) = app
        .drain_effects()
        .find_map(|e| match e {
            Effect::Provider { req: ProviderRequest::Op { id, ctl, .. }, .. } => Some((id, ctl)),
            _ => None,
        })
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let waiting = {
        let ctl = ctl.clone();
        std::thread::spawn(move || ctl.resolve(|| tx.send(()).unwrap()))
    };
    rx.recv().unwrap();
    let conflict: Conflict = archive::unreadable(&p(r"C:\unread\a.txt"), Meta::default());
    app.on_provider_event(ProviderEvent::OpConflict { tab: app.tab().id, id, conflict }, t0());
    assert_eq!(app.asking(), Some("unreadable"));
    key(&mut app, Key::Enter, NO);
    assert_eq!(waiting.join().unwrap(), Some(Resolve::Skip));
    assert_eq!(app.asking(), None);
}

#[test]
fn hashes_are_computed_in_one_request_and_compared_with_a_value_or_another_file() {
    let mut app = app_at(r"C:\hash");
    deliver(&mut app, &[("f.bin", FILE), ("sub", DIR)]);
    select(&mut app, &["sub"]);
    app.hashes_dialog();
    assert_eq!(app.asking(), None);
    select(&mut app, &["f.bin"]);
    app.run_action("file.hashes", t0());
    assert_eq!(app.asking(), Some("hashes"));
    let requests = |app: &mut App| app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Hash { id, file, kinds, .. }, .. } = e { Some((id, file, kinds)) } else { None }).collect::<Vec<_>>();
    let [(id, file, kinds)] = requests(&mut app).try_into().unwrap();
    assert_eq!((file, kinds.as_slice()), (p(r"C:\hash\f.bin"), &HashKind::ALL[..4]));
    let values: Vec<String> = ["c0ffee00", &"a".repeat(32), &"b".repeat(40), &"c".repeat(64)].iter().map(|s| s.to_string()).collect();
    app.job_progress(id, OpProgress { bytes: 50, total_bytes: 100, ..Default::default() }, t0());
    assert_eq!(form(&app).progress, Some(0.5));
    app.on_provider_event(ProviderEvent::Hashed { tab: app.tab().id, id, result: Ok(values.clone()) }, t0());
    assert_eq!(form(&app).progress, None);
    assert!(matches!(&form(&app).controls[H_VALUES + 3], Control::Value { value, .. } if *value == values[3]));
    key(&mut app, Key::Down, NO);
    typed(&mut app, &format!("SHA256: {}", "C".repeat(64)));
    key(&mut app, Key::Enter, NO);
    assert!(!form(&app).error && form(&app).note.contains("SHA-256"));
    app.jobs.form.as_mut().unwrap().set_text(H_COMPARE, &"d".repeat(40));
    key(&mut app, Key::Enter, NO);
    assert!(form(&app).error);
    app.jobs.form.as_mut().unwrap().set_text(H_COMPARE, r"C:\hash\copy.bin");
    key(&mut app, Key::Enter, NO);
    let [(other, file, _)] = requests(&mut app).try_into().unwrap();
    assert_eq!(file, p(r"C:\hash\copy.bin"));
    app.on_provider_event(ProviderEvent::Hashed { tab: app.tab().id, id: other, result: Ok(values) }, t0());
    assert!(!form(&app).error && form(&app).note.contains("identical"));
    if let Some(Control::Flags { items, .. }) = app.jobs.form.as_mut().unwrap().controls.get_mut(H_KINDS) {
        items[5].1 = true;
    }
    key(&mut app, Key::Enter, NO);
    let [(again, _, kinds)] = requests(&mut app).try_into().unwrap();
    assert_eq!(kinds.len(), 5);
    assert_eq!(form(&app).controls.len(), H_VALUES + 5);
    assert_eq!(app.persisted().archive.hashes.len(), 5);
    let ctl = match &form(&app).kind {
        Kind::Hashes(Hashing { run: Some(r), .. }) if r.id == again => r.ctl.clone(),
        _ => panic!("not running"),
    };
    key(&mut app, Key::Escape, NO);
    assert!(ctl.is_cancelled() && app.asking().is_none());
}

#[test]
fn every_dialog_draws_inside_its_frame_with_buttons_below_the_rows() {
    let mut app = app_at(r"C:\draw");
    deliver(&mut app, &[("a.zip", FILE), ("b.txt", FILE)]);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    let open: [fn(&mut App); 4] = [
        |a| a.compress_dialog(),
        |a| a.extract_dialog(),
        |a| a.hashes_dialog(),
        |a| a.ask_password(p(r"C:\draw\a.zip"), Retry::Reload(a.tab().id)),
    ];
    for (k, show) in open.iter().enumerate() {
        select(&mut app, &[if k == 2 { "b.txt" } else { "a.zip" }]);
        show(&mut app);
        assert!(app.jobs.form.is_some(), "dialog {k}");
        app.frame(&mut g, t0());
        let f = form(&app);
        let last = *f.rows.last().unwrap();
        assert!(f.rows.iter().all(|r| r.w > 0.0 && r.x >= 0.0 && r.right() <= 1000.0));
        assert!(f.button_rects.iter().all(|b| b.y >= last.bottom() && b.bottom() <= 700.0));
        for (c, row) in f.controls.iter().zip(&f.rows) {
            if let Control::Flags { items, spots, .. } = c {
                assert_eq!(spots.len(), items.len(), "dialog {k}");
                assert!(spots.iter().all(|s| s.x >= row.x && s.right() <= row.right() && s.bottom() <= row.bottom()), "dialog {k}: {spots:?} outside {row:?}");
            }
        }
        key(&mut app, Key::Escape, NO);
        assert!(app.jobs.form.is_none());
    }
}

#[test]
fn form_controls_answer_the_mouse() {
    let mut app = app_at(r"C:\mouse");
    deliver(&mut app, &[("a.txt", FILE)]);
    select(&mut app, &["a.txt"]);
    app.compress_dialog();
    app.layout();
    let centre = |r: Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let click = |app: &mut App, (x, y): (f32, f32)| {
        app.on_event(&Event::MouseMove { x, y }, t0());
        app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t0());
        app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, t0());
    };
    let names = centre(form(&app).rows[C_NAMES]);
    click(&mut app, names);
    assert!(form(&app).checked(C_NAMES) != app.archive.prefs.compression.encrypt_names);
    let format = form(&app).rows[C_FORMAT];
    click(&mut app, (format.right() - 4.0, format.y + format.h / 2.0));
    assert_eq!(form(&app).choice(C_FORMAT), 1);
    let cancel = centre(form(&app).button_rects[1]);
    click(&mut app, cancel);
    assert_eq!(app.asking(), None);
    assert!(ops(&mut app).is_empty());
}
