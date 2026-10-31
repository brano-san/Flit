use super::*;
use crate::render::kind;
use crate::text::tests::Mono;
use crate::ui::harness;

fn round_or_circle(i: &crate::render::Instance) -> bool {
    let [_, _, w, h] = i.rect;
    i.radius == 0.0 || ((w - h).abs() < 0.01 && (i.radius - w / 2.0).abs() < 0.01)
}

#[test]
fn round_corners_off_squares_popups_buttons_and_selections() {
    let mut a = harness::app(&["a.txt", "b.txt"]);
    a.open_palette();
    let mut g = Gfx::new(Mono, 1024);
    a.frame(&mut g, Instant::now());
    assert!(g.dl.instances.iter().any(|i| i.kind == kind::SOLID && !round_or_circle(i)), "rounded by default");
    let i = crate::ui::settings::index("appearance.round_corners").unwrap();
    assert!(crate::ui::SETTINGS[i].set(&mut a, crate::ui::Value::Bool(false)));
    a.frame(&mut g, Instant::now());
    assert!(g.dl.instances.iter().filter(|i| i.kind == kind::SOLID || i.kind == kind::SHADOW).all(round_or_circle), "only knobs and dots stay round");
}

#[test]
fn popups_fade_in_briefly_only_when_animations_are_on() {
    let mut a = harness::app(&["a.txt"]);
    let t0 = Instant::now();
    let mut g = Gfx::new(Mono, 1024);
    a.frame(&mut g, t0);
    a.open_palette();
    assert!(a.frame(&mut g, t0 + Duration::from_millis(1)), "the palette asks for frames while it fades in");
    a.frame(&mut g, t0 + Duration::from_millis(60));
    assert!(!a.frame(&mut g, t0 + Duration::from_millis(400)), "no frames once it is in place");
    let mut a = harness::app(&["a.txt"]);
    a.animations = false;
    a.frame(&mut g, t0);
    a.open_palette();
    assert!(!a.frame(&mut g, t0 + Duration::from_millis(1)), "with Animations off the palette appears at once");
}

#[test]
fn key_hints_and_buttons_take_their_colors_from_the_theme() {
    let t = crate::theme::DARK;
    let mut g = Gfx::new(Mono, 256);
    let mut p = Paint::new(&mut g, &t, 1.0);
    assert_eq!(p.fill_of(Button::Hover), Some(t.teal));
    assert_eq!(p.fill_of(Button::Open), Some(t.accent_fill));
    assert_eq!(p.fill_of(Button::Idle), None);
    assert_eq!(p.ink_of(Button::Off, t.text), t.text_dim.alpha(DISABLED));
    assert_eq!(Button::of(true, true, false), Button::Off);
    assert_eq!(Button::of(true, true, true), Button::Open);
    let one = p.keycaps_w(&["Up".to_owned()], crate::text::FontStyle::new(crate::text::FontFamily::Ui, 12.0, false));
    let two = p.keycaps_w(&["Up".to_owned(), "Up".to_owned()], crate::text::FontStyle::new(crate::text::FontFamily::Ui, 12.0, false));
    assert!(two > 2.0 * one, "caps keep a gap between them");
}

#[test]
fn key_hints_drop_the_first_ones_when_the_row_is_too_narrow() {
    let t = crate::theme::DARK;
    let font = crate::text::FontStyle::new(crate::text::FontFamily::Ui, 12.0, false);
    let items: [(&[&str], &str); 3] = [(&["Up", "Down"], "to navigate"), (&["Enter"], "to use"), (&["Escape"], "to dismiss")];
    for w in [600.0, 260.0, 40.0] {
        let r = crate::render::Rect::new(10.0, 0.0, w, 36.0);
        let mut g = Gfx::new(Mono, 256);
        Paint::new(&mut g, &t, 1.0).hints_within(r, &items, font);
        let glyphs: Vec<_> = g.dl.instances.iter().filter(|i| i.kind == kind::GLYPH).collect();
        assert_eq!(glyphs.is_empty(), w < 100.0, "only a row too narrow for any hint is left empty at {w}");
        assert!(glyphs.iter().all(|i| i.rect[0] >= r.x && i.rect[0] + i.rect[2] <= r.right()), "the hints fit in {w}");
    }
}

#[test]
fn a_focused_empty_field_draws_no_placeholder() {
    let t = crate::theme::DARK;
    let font = crate::text::FontStyle::new(crate::text::FontFamily::Ui, 12.0, false);
    let r = crate::render::Rect::new(10.0, 0.0, 300.0, 28.0);
    let glyphs = |focused: bool, text: &str| {
        let mut f = crate::ui::field::Field::default();
        f.edit.insert(text);
        let mut g = Gfx::new(Mono, 256);
        f.draw(&mut g, &t, r, font, focused, "Filter 8 files...", 1.0, None);
        g.dl.instances.iter().filter(|i| i.kind == kind::GLYPH).count()
    };
    assert!(glyphs(false, "") > 0, "an idle field shows its hint");
    assert_eq!(glyphs(true, ""), 0, "focus hides the hint before any typing");
    assert!(glyphs(true, "ab") > 0);
}
