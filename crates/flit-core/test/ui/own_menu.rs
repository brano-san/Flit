use super::*;
use crate::input::{Key, Mods, MouseButton};
use crate::provider::ProviderRequest;
use crate::text::tests::Mono;
use crate::ui::{Effect, Persisted};
use crate::{FsPath, Location};

fn cmd(label: &str) -> MenuItem {
    MenuItem::command(label, String::new())
}

fn app() -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

fn shown(app: &mut App, launches: Vec<Launch>) {
    let items = vec![cmd("Copy path"), MenuItem::Separator, MenuItem::Submenu { label: "Open with".into(), items: vec![cmd("Editor"), cmd("Viewer")] }];
    app.on_event(&Event::ShowMenu { items, x: 100.0, y: 100.0, launches, kind: crate::ui::MenuKind::None, shell: Vec::new(), pending: false }, Instant::now());
    let mut g = Gfx::new(Mono, 256);
    app.frame(&mut g, Instant::now());
}

fn key(app: &mut App, key: Key) {
    app.on_event(&Event::Key { key, mods: Mods::default(), pressed: true }, Instant::now());
}

#[test]
fn preedit_text_is_composed_inline_at_the_caret_until_it_is_committed() {
    use crate::input::Preedit;
    use crate::ui::field::{compose, EditLine};
    let line = EditLine { text: "ab", caret: 1, selection: 1..1 };
    let p = Preedit::new("にほ".into(), Some("に".len()));
    assert_eq!(compose(&line, &p), ("aにほb".to_owned(), 1 + "に".len(), 1..1 + "にほ".len()));
    assert_eq!(compose(&line, &"にほ".into()).1, 1 + "にほ".len(), "without a cursor the caret ends the preedit");
    assert_eq!(Preedit::new("にほ".into(), Some(1)).cursor, None, "a cursor inside a character is dropped");
    let mut app = app();
    let mut g = Gfx::new(Mono, 256);
    app.open_goto();
    app.on_event(&Event::Preedit(p.clone()), Instant::now());
    assert_eq!(app.preedit, p);
    app.frame(&mut g, Instant::now());
    app.on_event(&Event::Preedit(String::new().into()), Instant::now());
    assert!(app.preedit.is_empty());
    app.frame(&mut g, Instant::now());
}

#[test]
fn commands_are_counted_through_submenus() {
    assert_eq!(commands(&[cmd("a"), MenuItem::Separator, MenuItem::Submenu { label: "s".into(), items: vec![cmd("b"), cmd("c")] }]), 3);
}

#[test]
fn escape_and_clicks_outside_close_the_menu_without_a_command() {
    let mut app = app();
    shown(&mut app, Vec::new());
    assert!(app.own_menu_open());
    key(&mut app, Key::Escape);
    assert!(!app.own_menu_open());
    shown(&mut app, Vec::new());
    app.on_event(&Event::MouseDown { x: 900.0, y: 550.0, button: MouseButton::Left, clicks: 1 }, Instant::now());
    assert!(!app.own_menu_open());
    assert!(app.drain_effects().all(|e| matches!(e, Effect::Icon(_))));
}

#[test]
fn trailing_commands_launch_their_programs_and_the_rest_are_flit_commands() {
    let mut app = app();
    let editor = Launch::With { program: FsPath::parse("/usr/share/applications/editor.desktop").unwrap(), args: vec![FsPath::parse("/x/a.txt").unwrap()] };
    let viewer = Launch::With { program: FsPath::parse("/usr/share/applications/viewer.desktop").unwrap(), args: Vec::new() };
    shown(&mut app, vec![editor.clone(), viewer]);
    for k in [Key::Down, Key::Down, Key::Right, Key::Enter] {
        key(&mut app, k);
    }
    assert!(!app.own_menu_open());
    let launched: Vec<Launch> = app.drain_effects().filter_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } => Some(launch),
        _ => None,
    }).collect();
    assert_eq!(launched, [editor]);
    shown(&mut app, Vec::new());
    key(&mut app, Key::Down);
    key(&mut app, Key::Enter);
    assert!(!app.own_menu_open());
}

fn pending(app: &mut App) {
    let items = vec![cmd("Copy path"), cmd("Rename")];
    app.on_event(&Event::ShowMenu { items, x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::None, shell: Vec::new(), pending: true }, Instant::now());
    app.frame(&mut Gfx::new(Mono, 256), Instant::now());
}

fn late_shell_items(app: &mut App) {
    let items = vec![cmd("Edit with Notepad++"), MenuItem::Submenu { label: "Show more options".into(), items: vec![cmd("Send to")] }];
    app.on_event(&Event::ShellItems { items, shell: vec![7, 9] }, Instant::now());
    app.frame(&mut Gfx::new(Mono, 256), Instant::now());
}

fn picks(app: &mut App) -> Vec<Option<u32>> {
    app.drain_effects().filter_map(|e| if let Effect::ShellPick(p) = e { Some(p) } else { None }).collect()
}

#[test]
fn shell_items_that_arrive_later_join_the_open_menu_with_their_ids() {
    let mut app = app();
    pending(&mut app);
    late_shell_items(&mut app);
    assert_eq!(app.own_menu.as_ref().map(|m| m.items.len()), Some(5), "flit items, a separator, then the shell's");
    for k in [Key::Down, Key::Down, Key::Down, Key::Enter] {
        key(&mut app, k);
    }
    assert!(!app.own_menu_open());
    assert_eq!(picks(&mut app), [Some(7)]);
    pending(&mut app);
    late_shell_items(&mut app);
    for k in [Key::Down, Key::Down, Key::Enter] {
        key(&mut app, k);
    }
    assert_eq!(picks(&mut app), [None], "a Flit command releases the shell menu");
}

#[test]
fn windows_items_join_the_open_group_and_show_more_options_stays_last() {
    let mut app = app();
    let own = vec![cmd("Open"), MenuItem::Submenu { label: "Open in".into(), items: vec![cmd("New tab")] }, MenuItem::Separator, cmd("Copy")];
    app.on_event(&Event::ShowMenu { items: own, x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::File, shell: Vec::new(), pending: true }, Instant::now());
    app.frame(&mut Gfx::new(Mono, 256), Instant::now());
    let shell = vec![cmd("Terminal"), MenuItem::Separator, MenuItem::Submenu { label: "7-Zip".into(), items: vec![cmd("Add")] }, MenuItem::Submenu { label: "Show more options".into(), items: vec![cmd("Send to")] }];
    app.on_event(&Event::ShellItems { items: shell, shell: vec![4, 5, 6] }, Instant::now());
    app.frame(&mut Gfx::new(Mono, 256), Instant::now());
    let labels = app.context_menu_open().unwrap().labels();
    assert_eq!(labels, ["Open", "Open in >", "Terminal", "7-Zip >", "-", "Copy", "-", "Show more options >"]);
    for k in [Key::Down, Key::Down, Key::Down, Key::Enter] {
        key(&mut app, k);
    }
    assert!(!app.own_menu_open());
    assert_eq!(picks(&mut app), [Some(4)], "a lifted Windows item still runs its own verb");
}

#[test]
fn a_menu_still_waiting_for_shell_items_releases_them_and_ignores_late_ones() {
    let mut app = app();
    pending(&mut app);
    key(&mut app, Key::Escape);
    assert_eq!(picks(&mut app), [None]);
    late_shell_items(&mut app);
    assert!(!app.own_menu_open());
    shown(&mut app, Vec::new());
    late_shell_items(&mut app);
    assert_eq!(app.own_menu.as_ref().map(|m| m.items.len()), Some(3), "a menu without shell items takes none");
    pending(&mut app);
    pending(&mut app);
    assert_eq!(picks(&mut app), [None], "a new menu releases the one still waiting");
}

#[test]
fn a_right_click_on_another_file_closes_the_open_menu_and_asks_for_that_file() {
    let mut a = crate::ui::harness::app(&["a.txt", "b.txt", "c.txt"]);
    let mut g = Gfx::new(Mono, 256);
    let right = |a: &mut App, x: f32, y: f32| {
        a.on_event(&Event::MouseMove { x, y }, Instant::now());
        a.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, Instant::now());
        a.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, Instant::now());
        a.drain_effects().find_map(|e| match e {
            Effect::ShellMenu(m) => Some(m.items),
            _ => None,
        })
    };
    let (r0, r2) = (a.row_rect(0).unwrap(), a.row_rect(2).unwrap());
    let first = right(&mut a, r0.x + 700.0, r0.y + r0.h / 2.0).unwrap();
    assert_eq!(first, [FsPath::parse(r"C:\Work\a.txt").unwrap()]);
    a.on_event(&Event::ShowMenu { items: vec![cmd("Open")], x: r0.x + 700.0, y: r0.y, launches: Vec::new(), kind: crate::ui::MenuKind::File, shell: vec![1], pending: false }, Instant::now());
    a.frame(&mut g, Instant::now());
    let (x, y) = (r2.x + 60.0, r2.y + r2.h / 2.0);
    assert!(a.own_menu_open() && !a.over_own_menu(x, y));
    let second = right(&mut a, x, y);
    assert!(!a.own_menu_open(), "the old menu closes");
    assert_eq!(second, Some(vec![FsPath::parse(r"C:\Work\c.txt").unwrap()]), "the new menu is for the file under the pointer");
    let v = a.tab().view.as_ref().unwrap();
    assert!(v.is_selected(v.rows()[2]) && v.selected_count() == 1);
}

#[test]
fn a_menu_opens_at_the_size_its_kind_had_and_text_typed_early_survives_the_shell_items() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 256);
    let target = crate::ui::ShellMenu { dir: FsPath::parse(r"C:\x").unwrap(), items: vec![FsPath::parse(r"C:\x\a.TXT").unwrap()], x: 100.0, y: 100.0, extended: false, menu: Vec::new(), inline: false, shell: true, kind: crate::ui::MenuKind::File };
    assert_eq!(shape_key(&target), "file.txt");
    let mut open = |app: &mut App| {
        app.remember_shape_key(&target);
        app.on_event(&Event::ShowMenu { items: vec![cmd("Open")], x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::File, shell: Vec::new(), pending: true }, Instant::now());
        app.frame(&mut g, Instant::now());
        app.context_menu_open().map(|c| c.rect).unwrap()
    };
    let arrive = |app: &mut App| {
        let items = vec![cmd("Scan"), MenuItem::Separator, MenuItem::Submenu { label: "Show more options".into(), items: vec![cmd("Send to")] }];
        app.on_event(&Event::ShellItems { items, shell: vec![7, 9] }, Instant::now());
        app.frame(&mut Gfx::new(Mono, 256), Instant::now());
        app.context_menu_open().map(|c| c.rect).unwrap()
    };
    let unknown = open(&mut app);
    let full = arrive(&mut app);
    assert!(full.h > unknown.h, "the first menu of a kind grows when the shell joins");
    key(&mut app, Key::Escape);
    let early = open(&mut app);
    assert_eq!((early.h, early.w), (full.h, full.w), "the next one opens at the size it will have");
    assert_eq!(app.context_menu_open().unwrap().labels(), ["Open", "", "-", ""], "neutral rows hold the shell's places");
    app.on_event(&Event::Char('s'), Instant::now());
    let late = arrive(&mut app);
    let c = app.context_menu_open().unwrap();
    assert_eq!(c.query.text(), "s", "text typed before the shell items is kept");
    assert!(c.labels().iter().any(|l| l.contains("Scan")), "{:?}", c.labels());
    assert!(late.h <= full.h);
    assert_eq!(commands(&app.own_menu.as_ref().unwrap().items), 3, "no placeholder is a command");
}

#[test]
fn a_click_on_a_submenu_of_the_context_menu_redraws_it_open_at_once() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 256);
    let items = vec![cmd("Paste"), MenuItem::Submenu { label: "New".into(), items: vec![cmd("Folder"), cmd("Text Document")] }];
    app.on_event(&Event::ShowMenu { items, x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::Background, shell: Vec::new(), pending: false }, Instant::now());
    app.frame(&mut g, Instant::now());
    let c = app.context_menu_open().unwrap();
    let x = c.rect.x + c.rect.w / 2.0;
    let new_row = c.labels().iter().position(|l| l == "New >").unwrap();
    let y = (0..c.rect.h as i32).map(|dy| c.rect.y + dy as f32).find(|&y| c.hit(x, y) == Some(crate::ui::context::CtxHit::Row(new_row))).unwrap();
    for ev in [Event::MouseMove { x, y }, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }] {
        app.on_event(&ev, Instant::now());
        app.frame(&mut g, Instant::now());
    }
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
    assert!(app.context_menu_open().unwrap().is_open("New"));
    assert!(app.dirty, "the opened submenu is drawn on the next frame, not after the pointer moves again");
}

fn ms(t0: Instant, n: u64) -> Instant {
    t0 + std::time::Duration::from_millis(n)
}

fn settled_start() -> Instant {
    ms(Instant::now(), 2000)
}

fn numbered(n: usize) -> Vec<MenuItem> {
    (1..=n).map(|k| cmd(&format!("Item {k}"))).collect()
}

fn open_menu(app: &mut App, items: Vec<MenuItem>, launches: Vec<Launch>, (x, y): (f32, f32), kind: crate::ui::MenuKind, at: Instant) {
    app.on_event(&Event::ShowMenu { items, x, y, launches, kind, shell: Vec::new(), pending: false }, at);
}

fn drawn(app: &mut App, g: &mut Gfx<Mono>, at: Instant) -> (bool, Vec<crate::render::Instance>) {
    let moving = app.frame(g, at);
    let from = g.dl.instances.iter().rposition(|i| i.kind == crate::render::kind::SHADOW).unwrap();
    (moving, g.dl.instances[from..].to_vec())
}

fn fill(menu: &[crate::render::Instance]) -> Rect {
    let [x, y, w, h] = menu[1].rect;
    Rect::new(x, y, w, h)
}

fn glyphs_of_row(menu: &[crate::render::Instance], first: bool) -> Vec<usize> {
    let ys = menu.iter().filter(|i| i.kind == crate::render::kind::GLYPH).map(|i| i.rect[1]);
    let y = if first { ys.fold(f32::MAX, f32::min) } else { ys.fold(f32::MIN, f32::max) };
    let row: Vec<usize> = (0..menu.len()).filter(|&k| menu[k].kind == crate::render::kind::GLYPH && (menu[k].rect[1] - y).abs() < 1.0).collect();
    assert!(!row.is_empty());
    row
}

fn alpha(i: &crate::render::Instance) -> u32 {
    i.color >> 24
}

fn inside_the_panel(menu: &[crate::render::Instance]) -> bool {
    let f = fill(menu);
    menu[3..].iter().all(|i| i.clip[0] >= f.x - 1.0 && i.clip[1] >= f.y - 1.0 && i.clip[2] <= f.right() + 1.0 && i.clip[3] <= f.bottom() + 1.0)
}

#[test]
fn a_menu_below_the_pointer_unrolls_downward_and_its_items_drop_in_one_after_another() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 1024);
    let t0 = settled_start();
    open_menu(&mut app, numbered(6), Vec::new(), (100.0, 100.0), crate::ui::MenuKind::None, t0);
    let (moving_start, start) = drawn(&mut app, &mut g, t0);
    let (moving_mid, mid) = drawn(&mut app, &mut g, ms(t0, 90));
    let (moving_end, end) = drawn(&mut app, &mut g, ms(t0, 400));
    assert!(moving_start && moving_mid, "frames keep coming while the menu unrolls");
    assert!(!moving_end, "and stop once it is in place");
    let panel = app.menu_panels()[0];
    assert_eq!(fill(&end), panel.snap(), "it ends at its real place");
    assert_eq!(fill(&start).y, fill(&end).y, "it unrolls from the top edge, where the click was");
    assert!(fill(&start).h < fill(&end).h * 0.5 && fill(&start).h < fill(&mid).h && fill(&mid).h < fill(&end).h);
    let (first, last) = (glyphs_of_row(&end, true), glyphs_of_row(&end, false));
    for &k in &first {
        assert_eq!(alpha(&start[k]), 0, "nothing shows at its final place on the first frame");
        assert!(start[k].rect[1] < end[k].rect[1], "the first item drops in from above");
        assert!(alpha(&mid[k]) * 10 >= alpha(&end[k]) * 9 && (mid[k].rect[1] - end[k].rect[1]).abs() < 1.0, "the first item has landed by mid-way");
    }
    for &k in &last {
        assert_eq!(alpha(&start[k]), 0);
        assert!(start[k].clip[3] <= end[k].rect[1], "the last item is still rolled up at first");
        assert!(alpha(&mid[k]) > 0 && alpha(&mid[k]) < alpha(&end[k]), "the last item is still arriving mid-way");
    }
    assert!(inside_the_panel(&start) && inside_the_panel(&mid), "no item is drawn outside the unrolling panel");
}

#[test]
fn a_menu_flipped_above_the_pointer_unrolls_upward_from_its_bottom_edge() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 1024);
    let t0 = settled_start();
    open_menu(&mut app, numbered(6), Vec::new(), (100.0, 590.0), crate::ui::MenuKind::None, t0);
    let (_, start) = drawn(&mut app, &mut g, t0);
    let (_, mid) = drawn(&mut app, &mut g, ms(t0, 90));
    let (_, end) = drawn(&mut app, &mut g, ms(t0, 400));
    assert!(fill(&end).bottom() <= 590.0, "the menu opened above the pointer");
    assert_eq!(fill(&start).bottom(), fill(&end).bottom(), "it unrolls from the bottom edge");
    assert!(fill(&start).y > fill(&end).y + fill(&end).h * 0.5);
    let (top, bottom) = (glyphs_of_row(&end, true), glyphs_of_row(&end, false));
    for &k in &bottom {
        assert!(start[k].rect[1] > end[k].rect[1], "the item nearest the pointer rises in from below");
        assert!(alpha(&mid[k]) * 10 >= alpha(&end[k]) * 9, "and arrives first");
    }
    for &k in &top {
        assert!(start[k].clip[1] >= end[k].rect[1] + end[k].rect[3], "the far item is still rolled up at first");
        assert!(alpha(&mid[k]) < alpha(&end[k]), "and arrives last");
    }
    assert!(inside_the_panel(&start) && inside_the_panel(&mid));
}

#[test]
fn with_animations_off_a_menu_appears_at_once_exactly_where_it_ends_up() {
    let t0 = settled_start();
    let mut animated = app();
    let mut g = Gfx::new(Mono, 1024);
    open_menu(&mut animated, numbered(6), Vec::new(), (100.0, 100.0), crate::ui::MenuKind::None, t0);
    drawn(&mut animated, &mut g, t0);
    let (_, end) = drawn(&mut animated, &mut g, ms(t0, 400));
    let mut still = app();
    still.set_animations_wanted(false);
    let mut g = Gfx::new(Mono, 1024);
    open_menu(&mut still, numbered(6), Vec::new(), (100.0, 100.0), crate::ui::MenuKind::None, t0);
    let (moving, at_once) = drawn(&mut still, &mut g, t0);
    assert!(!moving, "no frames are asked for");
    assert_eq!(at_once.len(), end.len());
    for (a, b) in at_once.iter().zip(&end) {
        assert_eq!((a.rect, a.clip, a.color), (b.rect, b.clip, b.color));
    }
}

#[test]
fn clicks_and_keys_during_the_unrolling_act_on_the_items_real_places() {
    let launch = |name: &str| Launch::With { program: FsPath::parse(&format!("/usr/share/applications/{name}.desktop")).unwrap(), args: Vec::new() };
    let launches = vec![launch("one"), launch("two"), launch("three")];
    let launched = |app: &mut App| -> Vec<Launch> {
        app.drain_effects().filter_map(|e| match e {
            Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } => Some(launch),
            _ => None,
        }).collect()
    };
    let mut app = app();
    let mut g = Gfx::new(Mono, 1024);
    let t0 = settled_start();
    open_menu(&mut app, numbered(3), launches.clone(), (100.0, 100.0), crate::ui::MenuKind::None, t0);
    app.frame(&mut g, t0);
    assert!(app.frame(&mut g, ms(t0, 60)), "the menu is still unrolling");
    let panel = app.menu_panels()[0];
    let (x, y) = (panel.x + 40.0, panel.y + 4.0 + 28.0 + 14.0);
    app.on_event(&Event::MouseMove { x, y }, ms(t0, 61));
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, ms(t0, 62));
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, ms(t0, 63));
    assert!(!app.own_menu_open());
    assert_eq!(launched(&mut app), [launches[1].clone()], "the second item runs, not whatever is drawn under the pointer");
    let t1 = ms(t0, 1000);
    open_menu(&mut app, numbered(3), launches.clone(), (100.0, 100.0), crate::ui::MenuKind::None, t1);
    app.frame(&mut g, t1);
    for k in [Key::Down, Key::Down, Key::Down, Key::Enter] {
        key(&mut app, k);
    }
    assert_eq!(launched(&mut app), [launches[2].clone()], "keys walk the items before they have arrived");
}

#[test]
fn a_submenu_unrolls_from_the_side_it_opens_toward_while_its_parent_stays_put() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 1024);
    let t0 = settled_start();
    let items = vec![cmd("Copy path"), MenuItem::Separator, MenuItem::Submenu { label: "Open with".into(), items: vec![cmd("Editor"), cmd("Viewer")] }];
    open_menu(&mut app, items, Vec::new(), (100.0, 100.0), crate::ui::MenuKind::None, t0);
    app.frame(&mut g, t0);
    let (_, root) = drawn(&mut app, &mut g, ms(t0, 400));
    let panel = app.menu_panels()[0];
    let t1 = ms(t0, 500);
    app.on_event(&Event::MouseMove { x: panel.x + 40.0, y: panel.y + 4.0 + 28.0 + 9.0 + 14.0 }, t1);
    let (moving, start) = drawn(&mut app, &mut g, t1);
    assert_eq!(app.menu_panels().len(), 2, "the submenu opened");
    let shadows: Vec<usize> = (0..g.dl.instances.len()).filter(|&k| g.dl.instances[k].kind == crate::render::kind::SHADOW).collect();
    let parent = &g.dl.instances[shadows[shadows.len() - 2]..];
    assert_eq!(fill(parent), fill(&root), "the parent does not unroll again");
    let (_, end) = drawn(&mut app, &mut g, ms(t1, 400));
    assert!(moving);
    assert!(fill(&end).x > fill(&root).x, "the submenu opens to the right");
    assert_eq!(fill(&start).x, fill(&end).x, "it unrolls from its left edge");
    assert!(fill(&start).w < fill(&end).w * 0.5 && fill(&start).h == fill(&end).h);
    for k in glyphs_of_row(&end, true) {
        assert_eq!(alpha(&start[k]), 0);
        assert!(start[k].rect[0] < end[k].rect[0] && start[k].rect[1] == end[k].rect[1], "items slide in sideways");
    }
    assert!(inside_the_panel(&start));
}

#[test]
fn a_long_scrolling_menu_unrolls_as_quickly_as_a_short_one() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 1024);
    let t0 = settled_start();
    open_menu(&mut app, numbered(80), Vec::new(), (100.0, 100.0), crate::ui::MenuKind::None, t0);
    let (_, start) = drawn(&mut app, &mut g, t0);
    assert!(fill(&start).h < 600.0 * 0.5);
    assert!(inside_the_panel(&start));
    assert!(app.frame(&mut g, ms(t0, 150)));
    assert!(!app.frame(&mut g, ms(t0, 200)), "every item has arrived within a fifth of a second");
}

#[test]
fn a_context_menu_unrolls_and_rows_of_a_submenu_opened_inside_it_drop_in() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 1024);
    let t0 = settled_start();
    let items = vec![cmd("Paste"), MenuItem::Submenu { label: "New".into(), items: vec![cmd("Folder"), cmd("Text Document")] }];
    open_menu(&mut app, items, Vec::new(), (100.0, 100.0), crate::ui::MenuKind::Background, t0);
    let (_, start) = drawn(&mut app, &mut g, t0);
    let (moving, end) = drawn(&mut app, &mut g, ms(t0, 400));
    assert!(!moving);
    assert_eq!(fill(&start).y, fill(&end).y);
    assert!(fill(&start).h < fill(&end).h * 0.5 && inside_the_panel(&start));
    let c = app.context_menu_open().unwrap();
    let (paste, row) = (c.rects[0], c.rects[c.labels().iter().position(|l| l == "New >").unwrap()]);
    let (x, y) = (row.x + 40.0, row.y + row.h / 2.0);
    let t1 = ms(t0, 500);
    app.on_event(&Event::MouseMove { x, y }, t1);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t1);
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, t1);
    let (moving, opened) = drawn(&mut app, &mut g, t1);
    let (_, settled) = drawn(&mut app, &mut g, ms(t1, 400));
    assert!(moving, "the new rows ask for frames");
    assert_eq!(fill(&opened), fill(&settled), "the menu itself does not unroll again");
    let shift = settled.len() - opened.len();
    for k in glyphs_of_row(&settled, false) {
        assert_eq!(alpha(&opened[k - shift]), 0, "the last row of the opened submenu drops in");
        assert!(opened[k - shift].rect[1] < settled[k].rect[1]);
    }
    let kept: Vec<_> = settled.iter().filter(|i| i.kind == crate::render::kind::GLYPH && i.rect[1] >= paste.y && i.rect[1] < paste.bottom()).collect();
    assert!(!kept.is_empty());
    for i in kept {
        assert!(opened.iter().any(|o| (o.rect, o.color) == (i.rect, i.color)), "rows that were already there stay as they are");
    }
}

#[test]
fn a_menu_rebuilt_by_late_shell_items_keeps_unrolling_instead_of_starting_over() {
    let mut app = app();
    let mut g = Gfx::new(Mono, 1024);
    let t0 = settled_start();
    app.on_event(&Event::ShowMenu { items: vec![cmd("Open")], x: 100.0, y: 100.0, launches: Vec::new(), kind: crate::ui::MenuKind::File, shell: Vec::new(), pending: true }, t0);
    app.frame(&mut g, t0);
    app.frame(&mut g, ms(t0, 100));
    app.on_event(&Event::ShellItems { items: vec![cmd("Scan"), cmd("Share")], shell: vec![7, 9] }, ms(t0, 110));
    let (moving, rebuilt) = drawn(&mut app, &mut g, ms(t0, 120));
    assert!(moving);
    let full = app.context_menu_open().unwrap().rect;
    assert!(fill(&rebuilt).h > full.h * 0.8 && fill(&rebuilt).h < full.h, "it carries on from how far it had unrolled");
    assert!(!app.frame(&mut g, ms(t0, 200)), "and finishes when it would have");
}
