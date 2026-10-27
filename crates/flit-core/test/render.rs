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
