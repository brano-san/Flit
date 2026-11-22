use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::ui::{harness, App, Effect, Persisted, ShellMenu};
use crate::Location;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn file_list(unix_paths: bool) -> App {
    let path = Location::parse(if unix_paths { "/tmp/flit-context" } else { r"C:\flit-context" }).unwrap();
    let mut app = App::new(Persisted { path, ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    app.caps = crate::caps::Caps { unix_paths };
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Sensitive);
    for (name, attrs) in [("file.txt", 0), ("folder", attr::DIR)] {
        let i = listing.push(name.as_bytes(), Meta { attrs, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    let view = ViewSpec::default();
    sort::sort(&listing, &keys, view, &mut order);
    t.slot.publish(Arc::new(DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO }));
    app.poll();
    harness::frame(&mut app);
    app.drain_effects().for_each(drop);
    app
}

fn right_click(app: &mut App, at: (f32, f32)) -> ShellMenu {
    let (x, y) = at;
    let now = Instant::now();
    app.on_event(&Event::MouseMove { x, y }, now);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, now);
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, now);
    app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).expect("right click requests a menu")
}

fn row_for(app: &App, name: &[u8]) -> (f32, f32) {
    let v = app.tab().view.as_ref().unwrap();
    let row = v.rows().iter().position(|&i| v.listing().name(i) == name).unwrap();
    let r = app.row_rect(row).unwrap();
    (r.x + 45.0, r.y + r.h / 2.0)
}

fn menu_labels(items: &[MenuItem]) -> Vec<&str> {
    items
        .iter()
        .map(|item| match item {
            MenuItem::Command { label, .. } | MenuItem::Submenu { label, .. } | MenuItem::Header(label) => label.as_str(),
            MenuItem::Separator => "-",
        })
        .collect()
}

fn show_context(app: &mut App, menu: ShellMenu, shell: Vec<MenuItem>) -> Vec<String> {
    let now = Instant::now();
    let pending = !shell.is_empty();
    app.on_event(&Event::ShowMenu { items: menu.menu, x: menu.x, y: menu.y, launches: Vec::new(), kind: menu.kind, shell: Vec::new(), pending }, now);
    if pending {
        app.on_event(&Event::ShellItems { items: shell, shell: vec![1, 2] }, now);
    }
    harness::frame(app);
    app.context_menu_open().unwrap().labels()
}

#[test]
fn opening_and_editing_share_the_first_block_for_files_and_folders() {
    for unix_paths in [true, false] {
        for name in [b"file.txt".as_slice(), b"folder".as_slice()] {
            let mut app = file_list(unix_paths);
            let at = row_for(&app, name);
            let menu = right_click(&mut app, at);
            let shell = if unix_paths { Vec::new() } else { vec![MenuItem::Submenu { label: S::MenuOpenWith.t().to_owned(), items: vec![cmd("Editor")] }, cmd("Shell extension")] };
            let folder = menu.kind == MenuKind::Folder;
            let labels = show_context(&mut app, menu, shell);
            let mut expected = vec![S::MenuOpen.t().to_owned()];
            if folder {
                expected.push(format!("{} >", S::MenuOpenIn.t()));
            }
            if !unix_paths {
                expected.extend([format!("{} >", S::MenuOpenWith.t()), "Shell extension".to_owned()]);
            }
            expected.extend([S::MenuCut, S::MenuCopy].map(|s| s.t().to_owned()));
            if folder {
                expected.push(S::MenuPaste.t().to_owned());
            }
            expected.push(if unix_paths { S::MenuCopyPath } else { S::MenuCopyAsPath }.t().to_owned());
            expected.extend([S::MenuCreateShortcut, S::MenuRename, S::MenuDelete].map(|s| s.t().to_owned()));
            expected.push("-".to_owned());
            assert!(labels.starts_with(&expected), "{labels:?}");
        }
    }
}

#[test]
fn saved_menu_order_hidden_items_and_custom_action_keep_their_editor_rows() {
    let mut app = file_list(true);
    let settings = serde_json::json!({
        "menu.order_file": ["Inspect", S::MenuOpen.t(), S::MenuCopy.t(), S::MenuProperties.t()],
        "menu.hidden_file": [S::MenuRename.t()],
        "menu.custom": [{ "name": "Inspect", "command": "inspect {path}" }]
    });
    app.apply_doc(&crate::ui::settings::parse(&settings.to_string()));
    let at = row_for(&app, b"file.txt");
    let menu = right_click(&mut app, at);
    let labels = show_context(&mut app, menu, Vec::new());
    assert_eq!(labels[..3], ["Inspect", "-", S::MenuOpen.t()]);
    assert!(!labels.iter().any(|label| label == S::MenuRename.t()));
    let editor = app.menu_rows();
    assert_eq!(editor[0].label(), "Inspect");
    let position = |label| editor.iter().position(|row| row.label() == label).unwrap();
    assert!(position(S::MenuOpen.t()) < position(S::MenuCopy.t()));
    assert!(editor[position(S::MenuRename.t())].hidden);
}

#[test]
fn right_clicks_on_a_file_folder_and_background_request_distinct_menus() {
    for unix_paths in [true, false] {
        let mut app = file_list(unix_paths);
        let file_at = row_for(&app, b"file.txt");
        let folder_at = row_for(&app, b"folder");
        let empty_at = (file_at.0, app.layout.list.bottom() - 8.0);

        let file = right_click(&mut app, file_at);
        assert!(file.shell);
        assert_eq!(file.kind, MenuKind::File);
        assert_eq!(file.items, [file.dir.join(b"file.txt")]);
        let file_labels = menu_labels(&file.menu);
        assert!(file_labels.contains(&S::MenuOpen.t()) && file_labels.contains(&S::MenuRename.t()));
        assert!(!file_labels.contains(&S::MenuOpenIn.t()) && !file_labels.contains(&S::MenuNew.t()));

        let folder = right_click(&mut app, folder_at);
        assert!(folder.shell);
        assert_eq!(folder.kind, MenuKind::Folder);
        assert_eq!(folder.items, [folder.dir.join(b"folder")]);
        let open_in = folder
            .menu
            .iter()
            .find_map(|item| match item {
                MenuItem::Submenu { label, items } if label == S::MenuOpenIn.t() => Some(items),
                _ => None,
            })
            .unwrap();
        assert!(menu_labels(open_in).contains(&S::MenuOpenInNewTab.t()));

        let background = right_click(&mut app, empty_at);
        assert!(background.shell);
        assert_eq!(background.kind, MenuKind::Background);
        assert!(background.items.is_empty());
        assert_eq!(app.tab().view.as_ref().unwrap().selected_count(), 0);
        let background_labels = menu_labels(&background.menu);
        for label in [S::MenuNew, S::MenuPaste, S::MenusGroup] {
            assert!(background_labels.contains(&label.t()), "{background_labels:?}");
        }
        for label in [S::MenuCut, S::MenuCopy, S::MenuRename, S::MenuDelete, S::MenuOpenWith] {
            assert!(!background_labels.contains(&label.t()), "{background_labels:?}");
        }
    }
}
use super::super::menu::Cmd;

fn cmd(label: &str) -> MenuItem {
    MenuItem::command(label, String::new())
}

#[test]
fn opening_actions_are_together_and_properties_follow_platform_extensions() {
    let items = vec![
        cmd(S::MenuOpen.t()),
        MenuItem::Submenu { label: S::MenuOpenIn.t().into(), items: vec![cmd("New tab")] },
        MenuItem::Separator,
        cmd("Copy"),
        cmd(S::MenuProperties.t()),
        MenuItem::Separator,
        MenuItem::Submenu { label: S::MenuOpenWith.t().into(), items: vec![cmd("Editor")] },
        MenuItem::Submenu { label: "Scripts".into(), items: vec![cmd("Script")] },
    ];
    let mut c = ContextMenu::new(&items, MenuKind::File, vec![], (0.0, 0.0), Rect::new(0.0, 0.0, 600.0, 400.0), 1.0);
    c.set_roles(roles(&[Cmd::Action("item.open"), Cmd::Action("item.open_tab"), Cmd::Action("file.copy"), Cmd::Action("item.properties")]));
    c.lift_shell(6, false);
    let labels = c.labels();
    assert_eq!(&labels[..3], &[S::MenuOpen.t().to_owned(), format!("{} >", S::MenuOpenIn.t()), format!("{} >", S::MenuOpenWith.t())]);
    assert_eq!(labels.last().unwrap(), S::MenuProperties.t());
}

#[test]
fn items_that_only_share_a_label_with_flit_commands_stay_in_place() {
    let items = vec![cmd("Copy"), cmd(S::MenuOpen.t()), MenuItem::Separator, cmd("Rename"), cmd(S::MenuProperties.t())];
    let mut c = ContextMenu::new(&items, MenuKind::File, vec![], (0.0, 0.0), Rect::new(0.0, 0.0, 600.0, 400.0), 1.0);
    let user = Cmd::User(crate::provider::Launch::Run { program: "x".into(), args: String::new(), dir: None });
    c.set_roles(roles(&[Cmd::Action("file.copy"), user]));
    c.lift_shell(3, false);
    let labels = c.labels();
    assert_eq!(labels[..2], ["Copy".to_owned(), S::MenuOpen.t().to_owned()], "a user entry called Open is not moved to the top");
    assert_eq!(labels[2..], ["Rename".to_owned(), S::MenuProperties.t().to_owned()], "a shell extension's Properties keeps its place");
}

#[test]
fn menu_geometry_remains_valid_in_small_scaled_viewports() {
    for scale in [1.0, 1.5, 2.5] {
        let mut c = ContextMenu::new(&(0..50).map(|n| cmd(&format!("Long command {n}"))).collect::<Vec<_>>(), MenuKind::File, vec![], (180.0, 150.0), Rect::new(0.0, 0.0, 200.0, 160.0), scale);
        c.measure(&mut crate::text::Gfx::new(crate::text::tests::Mono, 256));
        assert!(c.rect.w <= 200.0 && c.rect.bottom() <= 160.0 && c.list.h >= 0.0);
        c.wheel(-100.0);
        assert!(c.scroll > 0.0);
    }
}

#[test]
fn translated_link_submenus_fit_a_narrow_window_and_keep_keyboard_focus_when_choices_arrive() {
    for lang in crate::i18n::Lang::ALL {
        let labels = [S::MenuShortcut, S::MenuLinksSymbolicLink, S::MenuLinksJunction, S::MenuLinksHardLink];
        let entries = labels
            .into_iter()
            .enumerate()
            .map(|(n, key)| {
                let mut item = cmd(key.in_lang(lang));
                if let MenuItem::Command { disabled, .. } = &mut item {
                    *disabled = n > 0;
                }
                item
            })
            .collect();
        let items = [MenuItem::Submenu { label: S::MenuLinksPasteAs.in_lang(lang).to_owned(), items: entries }];
        let mut c = ContextMenu::new(&items, MenuKind::Background, vec![], (250.0, 120.0), Rect::new(0.0, 0.0, 320.0, 280.0), 1.0);
        let mut gfx = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
        c.measure(&mut gfx);
        c.key(Key::Down, Mods::default());
        c.key(Key::Right, Mods::default());
        assert!(c.is_open(S::MenuLinksPasteAs.in_lang(lang)));
        c.key(Key::Down, Mods::default());
        assert_eq!(c.key(Key::Enter, Mods::default()).0, MenuOutcome::Command(0));
        let hot = c.hot;
        c.set_disabled(3, false);
        assert_eq!(c.hot, hot);
        c.key(Key::End, Mods::default());
        assert_eq!(c.key(Key::Enter, Mods::default()).0, MenuOutcome::Command(3));
        c.set_disabled(3, true);
        assert_eq!(c.key(Key::Enter, Mods::default()).0, MenuOutcome::Open);
        c.measure(&mut gfx);
        c.draw(&mut gfx, &crate::theme::LIGHT, true, &Default::default());
        assert!(c.rect.x >= 0.0 && c.rect.right() <= 320.0 && c.rect.bottom() <= 280.0);
        assert!(labels.into_iter().all(|key| c.labels().iter().any(|s| s.trim() == key.in_lang(lang))));
        assert_eq!(c.key(Key::Escape, Mods::default()).0, MenuOutcome::Closed);
    }
}

#[test]
fn long_menu_scrollbar_can_be_dragged_to_the_last_command() {
    let mut c = ContextMenu::new(&(0..80).map(|n| cmd(&format!("Item {n}"))).collect::<Vec<_>>(), MenuKind::File, vec![], (0.0, 0.0), Rect::new(0.0, 0.0, 600.0, 400.0), 1.0);
    let (x, y) = (c.thumb.x + c.thumb.w / 2.0, c.thumb.y + c.thumb.h / 2.0);
    assert_eq!(c.hit(x, y), Some(CtxHit::ScrollThumb));
    c.mouse_down(x, y, 1, false);
    c.mouse_move(x, c.track.bottom());
    assert!((c.scroll - (c.content_h - c.list.h)).abs() < 0.1);
    assert_eq!(c.mouse_up(x, c.track.bottom()), MenuOutcome::Open);
    assert!(c.grab.is_none());
}

fn items() -> Vec<MenuItem> {
    vec![cmd("Open"), MenuItem::Separator, MenuItem::Submenu { label: "View".into(), items: vec![cmd("Details"), cmd("Icons")] }, cmd("Delete")]
}

fn menu(pins: &[&str]) -> ContextMenu {
    ContextMenu::new(&items(), MenuKind::File, pins.iter().map(|p| (*p).to_owned()).collect(), (100.0, 100.0), Rect::new(0.0, 0.0, 1000.0, 800.0), 1.0)
}

fn press(c: &mut ContextMenu, key: Key) -> MenuOutcome {
    c.key(key, Mods::default()).0
}

#[test]
fn expanding_near_the_screen_edge_keeps_the_parent_under_the_pointer() {
    let mut c = ContextMenu::new(&[MenuItem::Submenu { label: "Create".into(), items: (0..40).map(|i| cmd(&format!("Item {i}"))).collect() }], MenuKind::File, Vec::new(), (800.0, 600.0), Rect::new(0.0, 0.0, 1000.0, 700.0), 1.0);
    c.measure(&mut crate::text::Gfx::new(crate::text::tests::Mono, 1024));
    let before = c.rect;
    let parent = c.rects[0];
    c.mouse_move(parent.x + 10.0, parent.y + 10.0);
    c.mouse_down(parent.x + 10.0, parent.y + 10.0, 1, false);
    c.mouse_up(parent.x + 10.0, parent.y + 10.0);
    assert_eq!((c.rect.x, c.rect.y), (before.x, before.y));
    assert_eq!(c.rects[0].y, parent.y);
    assert!(c.content_h > c.list.h);
    assert!(c.rect.bottom() <= 700.0);
}

#[test]
fn submenus_expand_inline_under_their_parent_and_collapse_again() {
    let mut c = menu(&[]);
    assert_eq!(c.labels(), ["Open", "-", "View >", "Delete"]);
    press(&mut c, Key::Down);
    press(&mut c, Key::Down);
    press(&mut c, Key::Right);
    assert!(c.is_open("View"));
    assert_eq!(c.labels(), ["Open", "-", "View v", "-", "  Details", "  Icons", "-", "Delete"]);
    press(&mut c, Key::Down);
    assert_eq!(press(&mut c, Key::Enter), MenuOutcome::Command(1), "commands keep their order through submenus");
    press(&mut c, Key::Left);
    assert!(!c.is_open("View"));
    assert_eq!(c.labels(), ["Open", "-", "View >", "Delete"]);
}

#[test]
fn typing_filters_every_command_including_submenus_and_escape_clears_before_closing() {
    let mut c = menu(&[]);
    c.insert("icn");
    assert_eq!(c.labels(), ["View \u{203A} Icons"]);
    assert_eq!(press(&mut c, Key::Enter), MenuOutcome::Command(2), "the best match is selected while typing");
    c.insert("zzz");
    assert!(c.labels().is_empty());
    assert_eq!(press(&mut c, Key::Escape), MenuOutcome::Open, "the first Escape clears the query");
    assert_eq!(c.labels(), ["Open", "-", "View >", "Delete"]);
    assert_eq!(press(&mut c, Key::Escape), MenuOutcome::Closed);
}

#[test]
fn pinned_commands_move_to_the_top_and_leave_their_place() {
    let mut c = menu(&["View \u{203A} Icons", "Delete", "Gone"]);
    assert_eq!(c.labels(), ["* View \u{203A} Icons", "* Delete", "-", "Open", "-", "View >"]);
    press(&mut c, Key::End);
    press(&mut c, Key::Right);
    assert_eq!(c.labels(), ["* View \u{203A} Icons", "* Delete", "-", "Open", "-", "View v", "-", "  Details"]);
    c.set_pins(Vec::new());
    assert_eq!(c.pins(), [] as [String; 0]);
    assert_eq!(c.labels()[0], "Open");
}

#[test]
fn the_star_of_a_command_asks_to_pin_it_and_of_a_pinned_one_to_unpin_it() {
    let mut c = menu(&["Delete"]);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 256);
    c.measure(&mut g);
    let row = c.rows.iter().position(|r| matches!(r, Row::Node { node, .. } if c.nodes[*node].label == "Open")).unwrap();
    let star = c.star_rect(row);
    let (x, y) = (star.x + star.w / 2.0, star.y + star.h / 2.0);
    c.mouse_move(x, y);
    c.mouse_down(x, y, 1, false);
    assert!(matches!(c.mouse_up(x, y), MenuOutcome::Pin(k, true, _) if k == "Open"));
    let star = c.star_rect(0);
    let (x, y) = (star.x + star.w / 2.0, star.y + star.h / 2.0);
    c.mouse_move(x, y);
    c.mouse_down(x, y, 1, false);
    assert!(matches!(c.mouse_up(x, y), MenuOutcome::Pin(k, false, _) if k == "Delete"));
}

#[test]
fn long_menus_scroll_inside_the_window() {
    let many: Vec<MenuItem> = (0..80).map(|k| cmd(&format!("Item {k}"))).collect();
    let mut c = ContextMenu::new(&many, MenuKind::Background, Vec::new(), (10.0, 10.0), Rect::new(0.0, 0.0, 600.0, 400.0), 1.0);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 256);
    c.measure(&mut g);
    assert!(c.rect.bottom() <= 400.0);
    let first = c.rects[0].y;
    c.wheel(-1.0);
    assert!(c.rects[0].y < first, "the wheel scrolls the list");
    press(&mut c, Key::End);
    let last = *c.rects.last().unwrap();
    assert!(last.bottom() <= c.list.bottom() + 0.5, "End reveals the last command");
}

#[test]
fn the_pin_sits_flush_with_the_row_edge_unless_a_mark_or_arrow_takes_it() {
    let mut c = menu(&[]);
    c.measure(&mut crate::text::Gfx::new(crate::text::tests::Mono, 256));
    assert_eq!(c.star_rect(0).right(), c.rects[0].right(), "a plain command has nothing at its edge");
    let view = c.labels().iter().position(|l| l == "View >").unwrap();
    assert_eq!(c.star_rect(view).right(), c.rects[view].right() - SIDE_W, "the submenu arrow keeps its column");
}

#[test]
fn hovering_the_pin_fades_its_button_in_and_settles() {
    let fade = crate::ui::widgets::HOVER_FADE;
    let mut c = menu(&[]);
    c.measure(&mut crate::text::Gfx::new(crate::text::tests::Mono, 256));
    let star = c.star_rect(0);
    let t0 = std::time::Instant::now();
    c.mouse_move(star.x + star.w / 2.0, star.y + star.h / 2.0);
    assert!(c.settle_glow(t0, true), "the pin button starts fading in");
    assert!(c.star_glow.alpha(0, t0 + fade / 2) < 1.0);
    assert!(!c.settle_glow(t0 + fade, true), "and stops asking for frames");
    assert_eq!(c.star_glow.alpha(0, t0 + fade), 1.0);
    c.mouse_move(c.rects[0].x + 40.0, c.rects[0].y + 4.0);
    assert!(!c.settle_glow(t0, false), "without animations the glow is instant");
    assert_eq!(c.star_glow.alpha(0, t0), 0.0);
}

#[test]
fn commands_with_bitmaps_upload_and_draw_them_in_the_icon_slot() {
    let image = MenuImage::pixels(crate::ui::Bitmap { width: 2, height: 2, rgba: vec![255; 16] });
    let key = image.key(16);
    let item = MenuItem::Command { label: "Scan".into(), keys: String::new(), checked: false, stay: false, mark: Mark::None, icon: "", image: Some(image), disabled: false };
    let mut c = ContextMenu::new(&[item], MenuKind::File, Vec::new(), (10.0, 10.0), Rect::new(0.0, 0.0, 600.0, 400.0), 1.0);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 256);
    c.measure(&mut g);
    let before = g.dl.instances.len();
    c.draw(&mut g, &crate::theme::DARK, true, &Default::default());
    assert!(g.has_image(key) && g.dl.instances.len() > before);
    assert_eq!(g.image_size(key), Some((2, 2)));
}

#[test]
fn rows_are_compact() {
    let mut c = menu(&[]);
    c.measure(&mut crate::text::Gfx::new(crate::text::tests::Mono, 256));
    assert_eq!(c.rects[0].h, 24.0);
    assert!(c.rect.w <= 240.0, "{}", c.rect.w);
}

#[test]
fn submenus_start_folded_stay_folded_under_the_pointer_and_open_on_a_click() {
    let items = vec![cmd("Open"), MenuItem::Submenu { label: "New".into(), items: vec![cmd("Folder"), MenuItem::Submenu { label: "More".into(), items: vec![cmd("Zip")] }] }, MenuItem::Submenu { label: "Send to".into(), items: vec![cmd("Desktop")] }];
    let mut c = ContextMenu::new(&items, MenuKind::Background, Vec::new(), (100.0, 100.0), Rect::new(0.0, 0.0, 1000.0, 800.0), 1.0);
    let x = c.rect.x + c.rect.w / 2.0;
    let row = |c: &ContextMenu, label: &str| {
        let k = c.labels().iter().position(|l| l.trim_start().trim_end_matches([' ', '>', 'v']) == label).unwrap();
        (0..c.rect.h as i32).map(|dy| c.rect.y + dy as f32).find(|&y| c.hit(x, y) == Some(CtxHit::Row(k))).unwrap()
    };
    assert_eq!(c.labels(), ["Open", "New >", "Send to >"], "everything starts folded");
    for label in ["New", "Send to"] {
        let y = row(&c, label);
        c.mouse_move(x, y);
    }
    assert_eq!(c.labels(), ["Open", "New >", "Send to >"], "passing over a submenu does not unfold it");
    let y = row(&c, "Send to");
    c.mouse_down(x, y, 1, false);
    c.mouse_up(x, y);
    assert!(c.is_open("Send to") && !c.is_open("New"), "a click unfolds it");
    c.mouse_down(x, y, 1, false);
    c.mouse_up(x, y);
    assert!(!c.is_open("Send to"), "the next click folds it");
}

#[test]
fn a_menu_that_fits_has_no_scroll_at_fractional_scales() {
    for scale in [0.9, 1.1, 1.25, 1.5, 1.75] {
        let mut c = ContextMenu::new(&items(), MenuKind::File, Vec::new(), (100.0, 100.0), Rect::new(0.0, 0.0, 1000.0, 800.0), scale);
        assert_eq!(c.arrows[0].w, 0.0, "no scroll arrows at {scale}");
        let before = c.rects[0].y;
        c.wheel(-5.0);
        assert_eq!(c.rects[0].y, before, "the wheel does not move a menu that fits at {scale}");
    }
}

#[test]
fn the_query_selects_by_dragging_and_by_whole_words_after_a_double_click() {
    let mut c = menu(&[]);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    c.measure(&mut g);
    c.insert("alpha beta");
    c.draw(&mut g, &crate::theme::DARK, true, &Default::default());
    let inner = c.field_inner();
    let y = inner.y + inner.h / 2.0;
    let (start, end) = (inner.x + 25.0, inner.x + 95.0);
    let mut picked = |c: &mut ContextMenu| {
        c.draw(&mut g, &crate::theme::DARK, true, &Default::default());
        c.query.edit.text[c.query.edit.selection()].to_owned()
    };
    assert!(matches!(c.mouse_down(end, y, 2, false), MenuOutcome::Open));
    assert_eq!(picked(&mut c), "beta");
    assert!(c.mouse_move(start, y));
    assert_eq!(picked(&mut c), "alpha beta", "dragging after a double click goes by words");
    assert!(matches!(c.mouse_up(start, y), MenuOutcome::Open), "a drag in the query never runs a command");
    c.mouse_move(end, y);
    assert_eq!(picked(&mut c), "alpha beta", "moving after the release changes nothing");
    c.mouse_down(start, y, 1, false);
    picked(&mut c);
    c.mouse_move(start + 20.0, y);
    assert_eq!(picked(&mut c), "ph", "after a single click it goes by characters");
    c.mouse_up(start + 20.0, y);
    c.mouse_down(end, y, 1, true);
    assert_eq!(picked(&mut c), "pha beta", "Shift+click extends");
    c.mouse_down(end, y, 3, false);
    assert_eq!(picked(&mut c), "alpha beta", "a triple click picks everything");
}

#[test]
fn icons_of_an_expanded_submenu_start_where_the_top_level_text_starts() {
    for scale in [1.0, 1.5] {
        let mut c = ContextMenu::new(&items(), MenuKind::File, Vec::new(), (10.0, 10.0), Rect::new(0.0, 0.0, 900.0, 700.0), scale);
        c.measure(&mut crate::text::Gfx::new(crate::text::tests::Mono, 256));
        press(&mut c, Key::Down);
        press(&mut c, Key::Down);
        press(&mut c, Key::Right);
        let labels = c.labels();
        let (view, details) = (labels.iter().position(|l| l == "View v").unwrap(), labels.iter().position(|l| l == "  Details").unwrap());
        let text_x = c.icon_slot(view).x + ICON_W * scale;
        let child = c.icon_slot(details);
        let image_x = child.x + (child.w - IMAGE_PX * scale) / 2.0;
        assert!((image_x - text_x).abs() < 0.01, "at {scale}x the child icon starts at {image_x}, the parent text at {text_x}");
        assert_eq!(c.icon_slot(view).x, c.rects[view].x, "top-level icons keep their place");
        assert!(child.x + ICON_W * scale >= image_x + IMAGE_PX * scale, "the child label stays clear of its icon");
    }
}
