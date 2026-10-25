use super::*;

#[test]
fn corrupted_widths_and_placement_fall_back_to_defaults() {
    let mut s = Saved::new(Persisted::default(), Some(Placement { x: 0, y: 0, width: -5, height: 300, maximized: false }));
    s.persisted.col_w = [f32::NAN, -10.0, 120.0];
    let s = s.sanitized();
    let d = Persisted::default().col_w;
    assert_eq!(s.persisted.col_w, [d[0], d[1], 120.0]);
    assert!(s.placement().is_none());
}
