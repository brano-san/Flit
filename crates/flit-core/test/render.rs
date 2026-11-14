use super::*;

#[test]
fn layout_and_colors() {
    assert_eq!(std::mem::size_of::<Instance>(), 64);
    assert_eq!(Color::rgb(0x112233).0.to_le_bytes(), [0x11, 0x22, 0x33, 0xFF]);
    assert_eq!(Color::rgb(0x112233).alpha(0x80).0.to_le_bytes(), [0x11, 0x22, 0x33, 0x80]);
    assert_eq!(Rect::new(0.4, 0.6, 9.2, 9.3).snap(), Rect::new(0.0, 1.0, 10.0, 9.0));
}

#[test]
fn a_check_mark_is_two_thick_strokes_inside_its_box() {
    for side in [14.0, 18.0, 21.0, 28.0, 36.0] {
        let r = Rect::new(10.5, 20.25, side, side);
        let mut dl = DrawList::default();
        dl.check(r, Color::rgb(0));
        let stroke = check_stroke(r);
        assert!(stroke >= side / 8.0 && stroke >= 1.75, "{side}: stroke {stroke}");
        assert!(dl.instances.len() >= 6);
        let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for i in &dl.instances {
            let [x, y, w, h] = i.rect;
            assert_eq!((w, h, i.radius, i.kind), (stroke, stroke, stroke / 2.0, kind::SOLID), "round dots of the stroke width");
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x + w), hi.1.max(y + h));
        }
        assert!(lo.0 >= r.x && lo.1 >= r.y && hi.0 <= r.right() && hi.1 <= r.bottom(), "{side}: the mark stays in the box");
        assert!(hi.0 - lo.0 >= side * 0.55 && hi.1 - lo.1 >= side * 0.45, "{side}: the mark fills the box");
        let gaps = dl.instances.windows(2).map(|p| (p[1].rect[0] - p[0].rect[0]).hypot(p[1].rect[1] - p[0].rect[1]));
        assert!(gaps.into_iter().all(|d| d <= stroke * 0.5 + 1e-3), "{side}: the strokes have no holes");
    }
}

#[test]
fn a_panel_refits_with_its_shadow_margin_and_slid_items_keep_their_clip() {
    let panel = Rect::new(10.0, 20.0, 100.0, 80.0);
    let mut dl = DrawList::default();
    dl.shadow(panel, Color::rgba(0x0000_0050), 6.0, 10.0);
    dl.round_rect(panel, Color::rgb(0xFFFFFF), 6.0);
    dl.refit(0, panel, Rect::new(10.0, 20.0, 100.0, 30.0));
    assert_eq!(dl.instances[0].rect, [0.0, 10.0, 120.0, 50.0], "the shadow keeps its blur around the shorter panel");
    assert_eq!(dl.instances[1].rect, [10.0, 20.0, 100.0, 30.0]);
    dl.refit(0, Rect::new(10.0, 20.0, 100.0, 30.0), Rect::new(10.0, 200.0, 100.0, 0.0));
    assert!(dl.instances.iter().all(|i| i.rect[2] >= 0.0 && i.rect[3] >= 0.0), "a size never goes negative");
    let mut dl = DrawList::default();
    dl.set_clip(Some(panel));
    dl.rect(Rect::new(20.0, 30.0, 10.0, 10.0), Color::rgb(0x102030).alpha(0x80));
    dl.nudge(0, 0.0, -8.0);
    dl.fade(0, 0.5);
    let i = dl.instances[0];
    assert_eq!((i.rect[1], i.clip), (22.0, [10.0, 20.0, 110.0, 100.0]), "a slide moves the item, not the panel's clip");
    assert_eq!(i.color >> 24, 0x40);
    assert_eq!(i.color & 0x00FF_FFFF, Color::rgb(0x102030).0 & 0x00FF_FFFF);
    dl.fade(0, 1.0);
    assert_eq!(dl.instances[0].color >> 24, 0x40, "a full fade leaves colours alone");
}
