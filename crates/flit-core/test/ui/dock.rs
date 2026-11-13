use super::*;

fn any(_: Dock) -> bool {
    true
}

fn docks(z: &[Zone]) -> Vec<Dock> {
    z.iter().map(|z| z.dock).collect()
}

fn between(z: &[Zone]) -> Option<Zone> {
    z.iter().find(|z| matches!(z.dock, Dock::Between { .. })).copied()
}

const WINDOW: Rect = Rect { x: 0.0, y: 0.0, w: 1200.0, h: 800.0 };

#[test]
fn a_wide_panel_offers_left_whole_and_right_in_a_row() {
    let z = zones(&[(0, WINDOW)], &[], WINDOW, (600.0, 400.0), 1.0, any);
    assert_eq!(docks(&z), [Dock::Split { panel: 0, down: false, before: true }, Dock::Into(0), Dock::Split { panel: 0, down: false, before: false }]);
    assert!(z[0].button.right() <= z[1].button.x && z[1].button.right() <= z[2].button.x);
    assert_eq!(z[0].button.y, z[2].button.y);
    assert_eq!(z[0].area, Rect::new(0.0, 0.0, 600.0, 800.0));
    assert_eq!(z[1].area, WINDOW);
    assert_eq!(z[2].area, Rect::new(600.0, 0.0, 600.0, 800.0));
    assert_eq!(zone_at(&z, z[1].button.x + 1.0, z[1].button.y + 1.0), Some(1));
    assert_eq!(zone_at(&z, 10.0, 10.0), None);
}

#[test]
fn a_tall_panel_offers_top_whole_and_bottom_in_a_column() {
    let tall = Rect::new(0.0, 0.0, 400.0, 800.0);
    let z = zones(&[(3, tall)], &[], tall, (200.0, 400.0), 1.0, any);
    assert_eq!(docks(&z), [Dock::Split { panel: 3, down: true, before: true }, Dock::Into(3), Dock::Split { panel: 3, down: true, before: false }]);
    assert_eq!(z[0].button.x, z[2].button.x);
    assert!(z[0].button.bottom() <= z[1].button.y);
    assert_eq!(z[2].area, Rect::new(0.0, 400.0, 400.0, 400.0));
}

#[test]
fn edges_near_the_cursor_add_halves_columns_and_junctions() {
    let left = Rect::new(0.0, 0.0, 600.0, 800.0);
    let right = Rect::new(601.0, 0.0, 599.0, 800.0);
    let panels = [(1, left), (2, right)];
    let seam = [Junction { line: Rect::new(600.0, 0.0, 1.0, 800.0), before: left, after: right, left: 1, right: 2 }];
    let top = zones(&panels, &seam, WINDOW, (300.0, 20.0), 1.0, any);
    assert_eq!(top.len(), 4);
    assert_eq!(top[3].dock, Dock::Split { panel: 1, down: true, before: true });
    assert_eq!(top[3].area, Rect::new(0.0, 0.0, 600.0, 400.0));
    assert!(top[3].button.y < 60.0);
    let bottom = zones(&panels, &seam, WINDOW, (300.0, 780.0), 1.0, any);
    assert_eq!(bottom[3].dock, Dock::Split { panel: 1, down: true, before: false });
    let edge = zones(&panels, &seam, WINDOW, (10.0, 300.0), 1.0, any);
    assert_eq!(edge[3].dock, Dock::Window { before: true });
    assert_eq!(edge[3].area, Rect::new(0.0, 0.0, 400.0, 800.0));
    assert_eq!(edge[3].scheme, Scheme::Third(0));
    let far = zones(&panels, &seam, WINDOW, (1190.0, 300.0), 1.0, any);
    assert_eq!(far[3].dock, Dock::Window { before: false });
    for x in [580.0, 620.0] {
        let j = between(&zones(&panels, &seam, WINDOW, (x, 300.0), 1.0, any)).expect("junction zone");
        assert_eq!(j.dock, Dock::Between { left: 1, right: 2 });
        assert_eq!(j.scheme, Scheme::Third(1));
        assert_eq!(j.area, Rect::new(400.0, 0.0, 401.0, 800.0), "the column the drop makes: a third of each side");
    }
    assert!(zones(&panels, &seam, WINDOW, (300.0, 300.0), 1.0, any).len() == 3, "no edge is near");
    assert!(zones(&panels, &seam, WINDOW, (-5.0, 300.0), 1.0, any).is_empty(), "outside the window");
}

#[test]
fn the_gap_between_two_panels_belongs_to_the_nearest_one() {
    let left = Rect::new(0.0, 0.0, 600.0, 800.0);
    let right = Rect::new(601.0, 0.0, 599.0, 800.0);
    let seam = [Junction { line: Rect::new(600.0, 0.0, 1.0, 800.0), before: left, after: right, left: 1, right: 2 }];
    let z = zones(&[(1, left), (2, right)], &seam, WINDOW, (600.5, 300.0), 1.0, any);
    assert!(z.iter().any(|z| z.dock == Dock::Into(1)), "{z:?}");
    assert_eq!(between(&z).map(|z| z.dock), Some(Dock::Between { left: 1, right: 2 }));
}

#[test]
fn a_junction_is_the_separator_under_the_cursor_with_its_full_length() {
    let left = Rect::new(0.0, 0.0, 600.0, 800.0);
    let column = Rect::new(601.0, 0.0, 599.0, 800.0);
    let top = Rect::new(601.0, 0.0, 599.0, 400.0);
    let low = Rect::new(601.0, 401.0, 299.0, 399.0);
    let low_right = Rect::new(901.0, 401.0, 299.0, 399.0);
    let panels = [(1, left), (2, top), (3, low), (4, low_right)];
    let seams = [
        Junction { line: Rect::new(600.0, 0.0, 1.0, 800.0), before: left, after: column, left: 1, right: 2 },
        Junction { line: Rect::new(900.0, 401.0, 1.0, 399.0), before: low, after: low_right, left: 3, right: 4 },
    ];
    for y in [100.0, 600.0] {
        let j = between(&zones(&panels, &seams, WINDOW, (590.0, y), 1.0, any)).expect("outer junction");
        assert_eq!(j.dock, Dock::Between { left: 1, right: 2 });
        assert_eq!((j.area.y, j.area.h), (0.0, 800.0), "the whole separator, not the half beside the cursor");
        assert!(j.button.contains(600.0, y), "the button follows the cursor along the separator");
    }
    let inner = between(&zones(&panels, &seams, WINDOW, (905.0, 600.0), 1.0, any)).expect("inner junction");
    assert_eq!(inner.dock, Dock::Between { left: 3, right: 4 });
    assert_eq!((inner.area.y, inner.area.bottom()), (401.0, 800.0));
    assert_eq!(between(&zones(&panels, &seams, WINDOW, (905.0, 200.0), 1.0, any)), None, "the lower separator does not reach the top panel");
    let low_end = between(&zones(&panels, &seams, WINDOW, (905.0, 405.0), 1.0, any)).expect("inner junction");
    assert!(low_end.button.y >= 401.0, "the button stays on its separator");
}

#[test]
fn refused_docks_are_left_out_and_the_rest_keep_their_places() {
    let all = zones(&[(0, WINDOW)], &[], WINDOW, (600.0, 400.0), 1.0, any);
    let z = zones(&[(0, WINDOW)], &[], WINDOW, (600.0, 400.0), 1.0, |d| d != Dock::Into(0));
    assert_eq!(docks(&z), [Dock::Split { panel: 0, down: false, before: true }, Dock::Split { panel: 0, down: false, before: false }]);
    assert_eq!((z[0].button, z[1].button), (all[0].button, all[2].button), "a button never slides under the cursor when Ctrl changes what is allowed");
    let edge = zones(&[(0, WINDOW)], &[], WINDOW, (10.0, 300.0), 1.0, |d| !matches!(d, Dock::Window { .. }));
    assert!(!edge.iter().any(|z| matches!(z.dock, Dock::Window { .. })));
    assert!(zones(&[(0, WINDOW)], &[], WINDOW, (600.0, 400.0), 1.0, |_| false).is_empty());
}

#[test]
fn schemes_draw_one_to_three_rects_with_the_target_filled() {
    let b = Rect::new(0.0, 0.0, 44.0, 32.0);
    assert_eq!(scheme_rects(b, Scheme::Whole, 1.0), [(Rect::new(6.0, 6.0, 32.0, 20.0), true)]);
    let halves = scheme_rects(b, Scheme::Half { down: false, before: false }, 1.0);
    assert_eq!(halves.iter().map(|r| r.1).collect::<Vec<_>>(), [false, true]);
    assert!(halves[0].0.right() < halves[1].0.x);
    let thirds = scheme_rects(b, Scheme::Third(1), 1.0);
    assert_eq!(thirds.iter().map(|r| r.1).collect::<Vec<_>>(), [false, true, false]);
}
