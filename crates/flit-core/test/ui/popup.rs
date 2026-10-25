use super::*;
use crate::text::tests::Mono;
use crate::theme;

fn cmd(label: &str) -> MenuItem {
    MenuItem::Command { label: label.into(), keys: String::new(), checked: false }
}

fn sample() -> Vec<MenuItem> {
    vec![
        cmd("Open"),
        MenuItem::Submenu { label: "Sort by".into(), items: vec![cmd("Name"), MenuItem::Separator, cmd("Size")] },
        MenuItem::Separator,
        MenuItem::Command { label: "Paste".into(), keys: "Ctrl+V".into(), checked: true },
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
    v.draw(&mut g, &theme::LIGHT, Color::rgba(0x00000030));
    let glyphs = g.dl.instances.iter().filter(|i| i.kind == crate::render::kind::GLYPH).count();
    let letters = "Open".len() + "Sort by".len() - 1 + "Paste".len() + "Ctrl+V".len() + 2;
    assert_eq!(glyphs, letters);
    let r = v.panels().next().unwrap();
    assert!(r.w >= LABEL_X + 50.0 + KEYS_GAP + 60.0 + ARROW_W);
}
