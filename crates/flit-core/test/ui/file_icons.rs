use super::*;
use crate::listing::{attr, Entry};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::{Bitmap, Effect, IconKind, IconResult, LayoutMode};

fn file_app(items: &[(&str, u32)]) -> App {
    let mut app = app_at("/tmp/flit-icons");
    publish(&mut app, ViewSpec::default(), true, items);
    app.drain_effects().for_each(drop);
    app
}

#[test]
fn file_emblems_cover_the_bottom_right_of_every_file_layout() {
    for mode in [LayoutMode::Details, LayoutMode::List, LayoutMode::Grid, LayoutMode::Cards, LayoutMode::Columns, LayoutMode::Tree, LayoutMode::Adaptive] {
        let mut app = file_app(&[("link.png", attr::LINK), ("locked.png", 1 << 11), ("plain.png", 0)]);
        app.set_layout(mode);
        let mut gfx = Gfx::new(Mono, 1024);
        app.frame(&mut gfx, Instant::now());
        let emblems: Vec<_> = app.drain_effects().filter_map(|e| match e {
            Effect::Icon(r) if format!("{:?}", r.kind).ends_with("Emblem") => Some(r),
            _ => None,
        }).collect();
        if cfg!(windows) {
            assert!(emblems.is_empty());
        } else {
            assert_eq!(emblems.len(), 2, "{mode:?} asks for both emblems");
            for req in emblems {
                app.on_icon(&mut gfx, req.key, IconResult::Ready(Bitmap { width: 1, height: 1, rgba: vec![255, 0, 0, 255] }));
            }
            app.frame(&mut gfx, Instant::now());
            assert!(gfx.dl.instances.iter().filter(|i| i.kind == crate::render::kind::IMAGE).count() >= 2, "{mode:?} draws both loaded emblems");
        }
    }
}

#[test]
fn content_icons_distinguish_names_and_modifications_without_changing_programs() {
    let file = Entry { size: 42, modified: 10, ..Entry::default() };
    let a = icon_key(b"image", b"", &file, 16, b"/tmp", 0);
    let b = icon_key(b"notes", b"", &file, 16, b"/tmp", 0);
    if cfg!(target_os = "linux") {
        assert_ne!(a.0, b.0);
        assert_ne!(a.0, icon_key(b"image", b"", &Entry { modified: 11, ..file }, 16, b"/tmp", 0).0);
        assert_eq!(format!("{:?}", a.1), "Content");
    } else {
        assert_eq!(a, b);
    }
    assert_eq!(icon_key(b"run", b"", &Entry { attrs: attr::EXEC, ..file }, 16, b"/tmp", 0).1, IconKind::Type);
    assert_eq!(icon_key(b"a.png", b"png", &file, 16, b"/tmp", 0), icon_key(b"b.png", b"png", &file, 16, b"/tmp", 0));
    assert_eq!(icon_key(b"empty", b"", &Entry::default(), 16, b"/tmp", 0).1, IconKind::Type);
    assert_eq!(icon_key(b"archived", b"", &Entry { attrs: attr::ARCHIVED, ..file }, 16, b"/tmp/archive.zip", 0).1, IconKind::Type);
}

#[test]
fn ten_thousand_extensionless_files_request_only_a_bounded_visible_batch() {
    let names: Vec<_> = (0..10_000).map(|i| format!("file{i:05}")).collect();
    let items: Vec<_> = names.iter().map(|n| (n.as_str(), 0)).collect();
    let mut app = file_app(&items);
    app.animations = false;
    let mut gfx = Gfx::new(Mono, 1024);
    for round in 0..4 {
        app.frame(&mut gfx, Instant::now());
        let reqs: Vec<_> = app.drain_effects().filter_map(|e| match e { Effect::Icon(r) if format!("{:?}", r.kind) == "Content" => Some(r), _ => None }).collect();
        if cfg!(target_os = "linux") {
            if round == 0 { assert!(!reqs.is_empty()); }
            assert!(reqs.len() <= 8);
            assert!(reqs.iter().all(|r| r.row < 100));
        } else {
            assert!(reqs.is_empty());
        }
        for req in reqs {
            app.on_icon(&mut gfx, req.key, IconResult::Ready(Bitmap { width: 1, height: 1, rgba: vec![0, 0, 255, 255] }));
        }
    }
}

#[test]
fn file_emblems_stay_inside_the_icon_and_share_only_their_theme_bitmap() {
    let icon = Rect::new(12.0, 20.0, 40.0, 40.0);
    let e = crate::ui::file_icons::emblems(attr::LINK, icon, 0);
    if cfg!(target_os = "linux") {
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].2, Rect::new(36.0, 44.0, 16.0, 16.0));
        assert_ne!(e[0].0, crate::ui::file_icons::emblems(attr::LINK, icon, 1)[0].0);
        let both = crate::ui::file_icons::emblems(attr::LINK | attr::UNREADABLE, icon, 0);
        assert_eq!(both.len(), 2);
        assert!(both.iter().all(|(_, _, r)| r.x >= icon.x && r.y >= icon.y && r.right() <= icon.right() && r.bottom() <= icon.bottom()));
    } else {
        assert!(e.is_empty());
    }
    assert!(crate::ui::file_icons::emblems(0, icon, 0).is_empty());
    assert!(crate::ui::file_icons::emblems(attr::READONLY, icon, 0).is_empty());
}

#[test]
fn thumbnails_keep_their_link_emblem_on_top() {
    let mut app = file_app(&[("link.png", attr::LINK)]);
    app.set_layout(LayoutMode::Grid);
    let mut gfx = Gfx::new(Mono, 1024);
    app.frame(&mut gfx, Instant::now());
    let requests: Vec<_> = app.drain_effects().collect();
    for e in requests {
        match e {
            Effect::Thumbnail(r) => app.on_thumb(&mut gfx, r.key, IconResult::Ready(Bitmap { width: 1, height: 1, rgba: vec![0, 255, 0, 255] })),
            Effect::Icon(r) if r.kind == IconKind::LinkEmblem => app.on_icon(&mut gfx, r.key, IconResult::Ready(Bitmap { width: 1, height: 1, rgba: vec![255, 0, 0, 255] })),
            _ => {}
        }
    }
    app.frame(&mut gfx, Instant::now());
    let thumb = gfx.dl.instances.iter().position(|i| i.kind == crate::render::kind::THUMB).expect("loaded thumbnail");
    if cfg!(target_os = "linux") {
        assert!(gfx.dl.instances[thumb + 1..].iter().any(|i| i.kind == crate::render::kind::IMAGE), "the emblem is above the thumbnail");
    }
}

#[test]
fn search_icons_use_real_file_stamps_and_show_emblems() {
    use crate::ui::{SearchEvent, SearchRow, SearchSource};
    let mut app = file_app(&[]);
    app.open_search();
    let gen = app.drain_effects().filter_map(|e| match e { Effect::Search(crate::ui::SearchRequest::Query { gen, .. }) => Some(gen), _ => None }).next_back().unwrap();
    app.on_search_event(SearchEvent::Found { gen, total: 2, folders: 0, done: true, source: SearchSource::Walk }, Instant::now());
    app.on_search_event(SearchEvent::Rows { gen, from: 0, rows: vec![SearchRow { name: b"image".to_vec(), dir: b"/tmp".to_vec(), meta: Meta { size: 42, modified: 7, ..Meta::default() } }, SearchRow { name: b"image-link".to_vec(), dir: b"/tmp".to_vec(), meta: Meta { size: 42, attrs: attr::LINK, ..Meta::default() } }] }, Instant::now());
    app.drain_effects().for_each(drop);
    let mut gfx = Gfx::new(Mono, 1024);
    app.frame(&mut gfx, Instant::now());
    let requests: Vec<_> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { Some(r) } else { None }).collect();
    if cfg!(target_os = "linux") {
        let content = requests.iter().find(|r| r.kind == IconKind::Content).expect("the nonempty result requests content type");
        assert!(app.visible_content_icons().contains(&content.key));
        assert!(requests.iter().any(|r| r.kind == IconKind::LinkEmblem));
    } else {
        assert!(!requests.iter().any(|r| matches!(r.kind, IconKind::Content | IconKind::LinkEmblem)));
    }
}

#[test]
fn columns_parent_rows_and_the_preview_header_draw_loaded_emblems() {
    let mut app = file_app(&[("link.png", attr::LINK), ("locked.png", attr::UNREADABLE)]);
    app.set_layout(LayoutMode::Columns);
    let mut gfx = Gfx::new(Mono, 1024);
    app.frame(&mut gfx, Instant::now());
    let snap = app.tab().view.as_ref().unwrap().snap().clone();
    for p in &mut app.tab_mut().panes {
        p.view = Some(crate::view::DirView::new(snap.clone()));
    }
    let theme = *app.theme();
    gfx.begin(theme.bg);
    app.draw_panes(&mut gfx, &theme);
    let requests: Vec<_> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { Some(r) } else { None }).collect();
    for r in requests.into_iter().filter(|r| matches!(r.kind, IconKind::LinkEmblem | IconKind::UnreadableEmblem)) {
        app.on_icon(&mut gfx, r.key, IconResult::Ready(Bitmap { width: 1, height: 1, rgba: vec![255, 0, 0, 255] }));
    }
    gfx.begin(theme.bg);
    app.draw_panes(&mut gfx, &theme);
    if cfg!(target_os = "linux") {
        assert!(gfx.dl.instances.iter().any(|i| i.kind == crate::render::kind::IMAGE), "parent columns show the loaded emblem");
    }
    app.set_layout(LayoutMode::Details);
    let v = app.tab_mut().view.as_mut().unwrap();
    let i = v.find(b"link.png").unwrap();
    v.set_selected(i, true);
    v.focus = Some(i);
    app.set_pane_state(crate::ui::PaneState { visible: true, ..Default::default() });
    app.frame(&mut gfx, Instant::now());
    let gen = app.drain_effects().find_map(|e| match e { Effect::Preview(r) if !r.details => Some(r.gen), _ => None }).unwrap();
    app.on_preview(gen, crate::preview::PreviewData::Basic { poster: None, note: None });
    app.frame(&mut gfx, Instant::now());
    let requests: Vec<_> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { Some(r) } else { None }).collect();
    if cfg!(target_os = "linux") {
        assert!(requests.iter().any(|r| r.kind == IconKind::LinkEmblem && r.size > 8), "preview requests a larger emblem");
    }
}

#[test]
fn one_content_budget_covers_both_file_panels_and_theme_changes_refresh_emblems() {
    let names: Vec<_> = (0..30).map(|i| format!("image{i:02}")).collect();
    let items: Vec<_> = names.iter().map(|n| (n.as_str(), 0)).collect();
    let mut app = file_app(&items);
    app.run_action("panel.open_right", Instant::now());
    publish(&mut app, ViewSpec::default(), true, &items);
    app.drain_effects().for_each(drop);
    let mut gfx = Gfx::new(Mono, 1024);
    app.frame(&mut gfx, Instant::now());
    let reqs: Vec<_> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { Some(r) } else { None }).collect();
    assert!(reqs.iter().filter(|r| r.kind == IconKind::Content).count() <= 8);
    let mut linked = file_app(&[("link.png", attr::LINK)]);
    linked.frame(&mut gfx, Instant::now());
    let first = linked.drain_effects().find_map(|e| match e { Effect::Icon(r) if r.kind == IconKind::LinkEmblem => Some(r.key), _ => None });
    linked.on_event(&Event::IconTheme, Instant::now());
    linked.frame(&mut gfx, Instant::now());
    let fresh = linked.drain_effects().find_map(|e| match e { Effect::Icon(r) if r.kind == IconKind::LinkEmblem => Some(r.key), _ => None });
    if cfg!(target_os = "linux") {
        assert!(first.is_some() && fresh.is_some());
        assert_ne!(first, fresh);
    } else {
        assert!(first.is_none() && fresh.is_none());
    }
}

#[test]
fn linux_folders_ask_for_their_own_icon_by_place_and_others_keep_the_shared_one() {
    let dir = Entry { attrs: attr::DIR, modified: 5, ..Entry::default() };
    let pictures = icon_key(b"Pictures", b"", &dir, 32, b"/home/ann", 0);
    let elsewhere = icon_key(b"Pictures", b"", &dir, 32, b"/media/usb", 0);
    let work = icon_key(b"Work", b"", &dir, 32, b"/home/ann", 0);
    if cfg!(target_os = "linux") {
        assert_eq!(pictures.1, IconKind::CustomFolder);
        assert_ne!(pictures.0, elsewhere.0);
        assert_ne!(pictures.0, work.0);
        assert_ne!(pictures.0, icon_key(b"Pictures", b"", &Entry { modified: 6, ..dir }, 32, b"/home/ann", 0).0, "a changed folder asks again");
    } else {
        assert_eq!(pictures, work);
        assert_eq!(pictures.1, IconKind::Folder);
    }
    if !cfg!(windows) {
        assert_eq!(icon_key(b"jump", b"", &Entry { attrs: attr::DIR | attr::LINK, ..dir }, 32, b"/home/ann", 0).1, IconKind::File);
    }
    assert_eq!(icon_key(b"usb", b"", &Entry { attrs: attr::DIR | attr::VOLUME, ..dir }, 32, b"/media", 0).1, IconKind::Volume);
    assert_eq!(icon_key(b"inner", b"", &Entry { attrs: attr::DIR | attr::ARCHIVED, ..dir }, 32, b"/tmp/a.zip", 0).1, IconKind::Folder);
    assert_eq!(icon_key(b"gone", b"", &Entry { attrs: attr::DIR | attr::DELETED, ..dir }, 32, b"/home/ann", 0).1, IconKind::Folder);
}

#[test]
fn folders_waiting_for_their_own_icon_show_the_shared_folder_icon_in_every_layout() {
    let red = || IconResult::Ready(Bitmap { width: 1, height: 1, rgba: vec![255, 0, 0, 255] });
    for mode in [LayoutMode::Details, LayoutMode::Grid, LayoutMode::Columns, LayoutMode::Tree] {
        let mut app = file_app(&[("Pictures", attr::DIR), ("Work", attr::DIR)]);
        app.set_layout(mode);
        let mut gfx = Gfx::new(Mono, 1024);
        app.frame(&mut gfx, Instant::now());
        let reqs: Vec<IconRequest> = app.drain_effects().filter_map(|e| match e { Effect::Icon(r) => Some(r), _ => None }).collect();
        let own: Vec<&IconRequest> = reqs.iter().filter(|r| r.kind == IconKind::CustomFolder).collect();
        let row_size = own.first().map_or(0, |r| r.size);
        let is_shared = |r: &IconRequest| r.kind == IconKind::Folder && r.size == row_size;
        let shared = reqs.iter().filter(|r| is_shared(r)).count();
        if !cfg!(target_os = "linux") {
            assert!(own.is_empty(), "{mode:?}");
            continue;
        }
        assert_eq!(shared, 1, "{mode:?} asks for the shared folder icon at the row size once");
        let named: Vec<&[u8]> = own.iter().map(|r| r.path.file_name()).collect();
        assert!(named.contains(&&b"Pictures"[..]) && named.contains(&&b"Work"[..]), "{mode:?}: {named:?}");
        let images = |app: &mut App, gfx: &mut Gfx<Mono>| {
            app.frame(gfx, Instant::now());
            gfx.dl.instances.iter().filter(|i| i.kind == crate::render::kind::IMAGE).count()
        };
        for r in reqs.iter().filter(|r| r.kind == IconKind::CustomFolder) {
            let result = if r.path.file_name() == b"Pictures" { red() } else { IconResult::Failed };
            app.on_icon(&mut gfx, r.key, result);
        }
        let without_shared = images(&mut app, &mut gfx);
        assert!(without_shared >= 1, "{mode:?}: the custom icon is drawn");
        for r in reqs.iter().filter(|r| is_shared(r)) {
            app.on_icon(&mut gfx, r.key, red());
        }
        app.drain_effects().for_each(drop);
        assert!(images(&mut app, &mut gfx) > without_shared, "{mode:?}: the folder without its own icon shows the shared one");
        assert!(app.drain_effects().all(|e| !matches!(e, Effect::Icon(ref r) if r.kind == IconKind::CustomFolder)), "{mode:?}: answered folders are not asked again");
    }
}
