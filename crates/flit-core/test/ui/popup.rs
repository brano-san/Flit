use super::*;
use crate::render::Color;
use crate::text::tests::Mono;
use crate::theme;

fn cmd(label: &str) -> MenuItem {
    MenuItem::command(label, String::new())
}

fn sample() -> Vec<MenuItem> {
    vec![
        cmd("Open"),
        MenuItem::Submenu { label: "Sort by".into(), items: vec![cmd("Name"), MenuItem::Separator, cmd("Size")] },
        MenuItem::Separator,
        MenuItem::command("Paste", "Ctrl+V").checked(true),
    ]
}

fn view(x: f32, y: f32, bounds: Rect) -> MenuView {
    let mut g = Gfx::new(Mono, 256);
    MenuView::new(&sample(), x, y, bounds, 1.0, &mut g)
}

fn screen() -> Rect {
    Rect::new(0.0, 0.0, 1000.0, 800.0)
}

fn center(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn row(v: &MenuView, panel: usize, i: usize) -> (f32, f32) {
    center(v.rows(&v.panels[panel]).nth(i).unwrap().1)
}

#[test]
fn a_click_on_a_toggle_flips_its_check_in_place_and_keeps_the_submenu_open() {
    let toggle = |label: &str, checked| MenuItem::command(label, String::new()).checked(checked).staying();
    let items = vec![cmd("Open"), MenuItem::Submenu { label: "Sections".into(), items: vec![toggle("Pinned", true), toggle("Drives", false)] }];
    let mut v = MenuView::new(&items, 100.0, 100.0, screen(), 1.0, &mut Gfx::new(Mono, 256));
    let (x, y) = row(&v, 0, 1);
    v.mouse_move(x, y);
    let (x, y) = row(&v, 1, 0);
    v.mouse_move(x, y);
    assert_eq!(v.mouse_up(x, y), MenuOutcome::Toggled(1));
    assert_eq!(v.panels().count(), 2, "the submenu stays open");
    assert!(matches!(v.lists[1].nodes[0], Node::Command { checked: false, .. }));
    assert_eq!(v.mouse_up(x, y), MenuOutcome::Toggled(1));
    assert!(matches!(v.lists[1].nodes[0], Node::Command { checked: true, .. }));
    assert_eq!(v.key(Key::Space), MenuOutcome::Toggled(1));
    let long = vec![cmd("A label long enough to set the width of its panel")];
    let mut g = Gfx::new(Mono, 256);
    let w = MenuView::new(&long, 0.0, 0.0, screen(), 1.0, &mut g).lists[0].width;
    let room = w - PAD - LABEL_X - ARROW_W;
    assert!(room >= g.measure("A label long enough to set the width of its panel", font(1.0)), "the widest label is not cut by the row padding");
    let (x, y) = row(&v, 0, 0);
    v.mouse_move(x, y);
    assert_eq!(v.mouse_up(x, y), MenuOutcome::Command(0));
}

#[test]
fn commands_are_numbered_depth_first_like_native_menus() {
    let mut v = view(100.0, 100.0, screen());
    let (x, y) = row(&v, 0, 1);
    assert!(v.mouse_move(x, y), "hovering a submenu opens it");
    assert_eq!(v.panels().count(), 2);
    let (x, y) = row(&v, 1, 2);
    v.mouse_move(x, y);
    assert_eq!(v.mouse_up(x, y), MenuOutcome::Command(2));
    let (x, y) = row(&v, 0, 3);
    v.mouse_move(x, y);
    assert_eq!(v.panels().count(), 1, "hovering a plain row closes the submenu");
    assert_eq!(v.mouse_up(x, y), MenuOutcome::Command(3));
    let (x, y) = row(&v, 0, 2);
    assert_eq!(v.mouse_up(x, y), MenuOutcome::Open, "separators do nothing");
}

#[test]
fn panels_stay_inside_the_bounds() {
    let b = screen();
    let v = view(990.0, 790.0, b);
    let root = v.panels().next().unwrap();
    assert!(root.right() <= b.right() && root.bottom() <= b.bottom() && root.x >= 0.0 && root.y >= 0.0);
    assert!(root.right() <= 990.0, "the menu flips to the left of the pointer");
    assert!(root.bottom() <= 790.0, "the menu flips above the pointer");
    let mut v = view(900.0, 10.0, b);
    let (x, y) = row(&v, 0, 1);
    v.mouse_move(x, y);
    let panels: Vec<Rect> = v.panels().collect();
    assert!(panels[1].right() <= panels[0].x + 2.0 * PAD, "a submenu that does not fit on the right opens on the left");
    assert!(MIN_W <= panels[0].w);
}

#[test]
fn the_keyboard_walks_rows_and_submenus() {
    let mut v = view(100.0, 100.0, screen());
    assert_eq!(v.key(Key::Down), MenuOutcome::Open);
    assert_eq!(v.panels[0].hot, Some(0));
    v.key(Key::Down);
    v.key(Key::Down);
    assert_eq!(v.panels[0].hot, Some(3), "separators are skipped");
    v.key(Key::Down);
    assert_eq!(v.panels[0].hot, Some(0), "moving wraps around");
    v.key(Key::Char('s'));
    assert_eq!(v.panels[0].hot, Some(1));
    v.key(Key::Right);
    assert_eq!(v.panels().count(), 2);
    assert_eq!(v.panels[1].hot, Some(0));
    v.key(Key::End);
    assert_eq!(v.key(Key::Enter), MenuOutcome::Command(2));
    assert_eq!(v.key(Key::Left), MenuOutcome::Open);
    assert_eq!(v.panels().count(), 1);
    assert_eq!(v.key(Key::Escape), MenuOutcome::Closed);
}

#[test]
fn a_click_outside_closes_and_the_opening_release_is_ignored() {
    let mut v = view(100.0, 100.0, screen());
    let (x, y) = row(&v, 0, 0);
    assert_eq!(v.mouse_up(x, y), MenuOutcome::Open, "the button release that opened the menu does not pick a row");
    assert_eq!(v.mouse_down(x, y), MenuOutcome::Open);
    assert_eq!(v.mouse_up(x, y), MenuOutcome::Command(0));
    assert!(v.contains(x, y));
    assert_eq!(v.mouse_down(5.0, 5.0), MenuOutcome::Closed);
}

#[test]
fn draws_labels_keys_checks_and_arrows() {
    let mut g = Gfx::new(Mono, 512);
    let mut v = MenuView::new(&sample(), 10.0, 10.0, screen(), 1.0, &mut g);
    v.key(Key::Down);
    g.begin(Color::default());
    v.draw(&mut g, &theme::LIGHT);
    let glyphs = g.dl.instances.iter().filter(|i| i.kind == crate::render::kind::GLYPH).count();
    let letters = "Open".len() + "Sort by".len() - 1 + "Paste".len() + "Ctrl+V".len() + 1;
    assert!(g.dl.instances.iter().any(|i| i.color == theme::LIGHT.accent_fill.0), "the checked row has a filled box and the hot row an accent fill");
    assert_eq!(glyphs, letters);
    let r = v.panels().next().unwrap();
    assert!(r.w >= LABEL_X + 50.0 + KEYS_GAP + 60.0 + ARROW_W);
}

#[test]
fn headers_and_disabled_commands_are_shown_but_never_picked() {
    let later = MenuItem::Command { label: "Later".into(), keys: "Ctrl+E".into(), checked: false, stay: false, mark: Mark::None, icon: "\u{E8B7}", disabled: true };
    let items = vec![MenuItem::Header("Folder".into()), later, cmd("Open")];
    let mut v = MenuView::new(&items, 100.0, 100.0, screen(), 1.0, &mut Gfx::new(Mono, 256));
    assert_eq!(v.key(Key::Down), MenuOutcome::Open);
    assert_eq!(v.panels[0].hot, Some(2), "the keyboard skips the header and the gray command");
    assert_eq!(v.key(Key::Enter), MenuOutcome::Command(1));
    for i in 0..2 {
        let (x, y) = row(&v, 0, i);
        v.mouse_move(x, y);
        assert_ne!(v.panels[0].hot, Some(i));
        assert!(!matches!(v.mouse_up(x, y), MenuOutcome::Command(_)), "row {i}");
    }
}
