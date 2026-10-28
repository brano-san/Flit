use super::*;

fn docks(z: &[Zone]) -> Vec<Dock> {
    z.iter().map(|z| z.dock).collect()
}

const WINDOW: Rect = Rect { x: 0.0, y: 0.0, w: 1200.0, h: 800.0 };

#[test]
fn a_wide_panel_offers_left_whole_and_right_in_a_row() {
    let z = zones(&[(0, WINDOW)], WINDOW, (600.0, 400.0), 1.0);
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
    let z = zones(&[(3, tall)], tall, (200.0, 400.0), 1.0);
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
    let top = zones(&panels, WINDOW, (300.0, 20.0), 1.0);
    assert_eq!(top.len(), 4);
    assert_eq!(top[3].dock, Dock::Split { panel: 1, down: true, before: true });
    assert_eq!(top[3].area, Rect::new(0.0, 0.0, 600.0, 400.0));
    assert!(top[3].button.y < 60.0);
    let bottom = zones(&panels, WINDOW, (300.0, 780.0), 1.0);
    assert_eq!(bottom[3].dock, Dock::Split { panel: 1, down: true, before: false });
    let edge = zones(&panels, WINDOW, (10.0, 300.0), 1.0);
    assert_eq!(edge[3].dock, Dock::Window { before: true });
    assert_eq!(edge[3].area, Rect::new(0.0, 0.0, 400.0, 800.0));
    assert_eq!(edge[3].scheme, Scheme::Third(0));
    let far = zones(&panels, WINDOW, (1190.0, 300.0), 1.0);
    assert_eq!(far[3].dock, Dock::Window { before: false });
    let from_left = zones(&panels, WINDOW, (580.0, 300.0), 1.0);
    let from_right = zones(&panels, WINDOW, (620.0, 300.0), 1.0);
    for z in [&from_left, &from_right] {
        let j = z.iter().find(|z| matches!(z.dock, Dock::Between { .. })).expect("junction zone");
        assert_eq!(j.dock, Dock::Between { left: 1 });
        assert_eq!(j.scheme, Scheme::Third(1));
        assert!(j.area.x < 601.0 && j.area.right() > 601.0);
    }
    assert!(zones(&panels, WINDOW, (300.0, 300.0), 1.0).len() == 3, "no edge is near");
    assert!(zones(&panels, WINDOW, (-5.0, 300.0), 1.0).is_empty(), "outside every panel");
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
