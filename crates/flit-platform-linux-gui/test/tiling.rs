use super::*;

const ATOMS: StateAtoms = StateAtoms { vertical: 301, horizontal: 302, fullscreen: 303 };
const SCREEN: Area = Area { x: 0, y: 32, width: 1920, height: 1048 };

#[test]
fn mutter_edge_constraints_lock_only_the_sides_against_the_monitor() {
    let free_window = TOP_RESIZABLE | RIGHT_RESIZABLE | BOTTOM_RESIZABLE | LEFT_RESIZABLE;
    assert_eq!(free_window, 170);
    assert_eq!(gtk_edge_constraints(free_window), Sides::ALL);
    let left_half = TOP_TILED | RIGHT_RESIZABLE | BOTTOM_TILED | LEFT_TILED;
    assert_eq!(gtk_edge_constraints(left_half), Sides { top: false, right: true, bottom: false, left: false });
    let left_half_next_to_another = TOP_TILED | RIGHT_TILED | RIGHT_RESIZABLE | BOTTOM_TILED | LEFT_TILED;
    assert!(gtk_edge_constraints(left_half_next_to_another).right, "the edge shared with a tiled neighbour still resizes both");
    let maximized = TOP_TILED | RIGHT_TILED | BOTTOM_TILED | LEFT_TILED;
    assert_eq!(gtk_edge_constraints(maximized), Sides::NONE);
}

#[test]
fn net_wm_state_locks_the_maximized_axis_and_everything_in_fullscreen() {
    assert_eq!(net_wm_state(&[], ATOMS), Sides::ALL);
    assert_eq!(net_wm_state(&[999, ATOMS.vertical], ATOMS), Sides { top: false, bottom: false, left: true, right: true });
    assert_eq!(net_wm_state(&[ATOMS.horizontal], ATOMS), Sides { top: true, bottom: true, left: false, right: false });
    assert_eq!(net_wm_state(&[ATOMS.vertical, ATOMS.horizontal], ATOMS), Sides::NONE);
    assert_eq!(net_wm_state(&[ATOMS.fullscreen], ATOMS), Sides::NONE);
}

#[test]
fn the_workarea_of_the_current_desktop_decides_which_sides_touch_the_screen() {
    let areas = [0, 32, 1920, 1048, 0, 0, 1920, 1080];
    assert_eq!(workarea(&areas, Some(0)), Some(SCREEN));
    assert_eq!(workarea(&areas, Some(1)), Some(Area { x: 0, y: 0, width: 1920, height: 1080 }));
    assert_eq!(workarea(&areas, Some(7)), Some(SCREEN), "an unknown desktop falls back to the first area");
    assert_eq!(workarea(&areas, None), Some(SCREEN));
    assert_eq!(workarea(&[0, 0, 0, 0], None), None);
    assert_eq!(workarea(&[1, 2, 3], None), None);
    let quick_tiled_left = Area { x: 0, y: 32, width: 960, height: 1048 };
    assert_eq!(against_workarea(quick_tiled_left, SCREEN), Sides { top: false, left: false, bottom: false, right: true });
    let floating = Area { x: 100, y: 100, width: 800, height: 600 };
    assert_eq!(against_workarea(floating, SCREEN), Sides::ALL);
    let top_right_quarter = Area { x: 960, y: 32, width: 960, height: 524 };
    assert_eq!(against_workarea(top_right_quarter, SCREEN), Sides { top: false, right: false, bottom: true, left: true });
}

#[test]
fn x11_sides_trusts_mutter_constraints_over_geometry_and_adds_the_wm_state() {
    let floating = Area { x: 100, y: 100, width: 800, height: 600 };
    let at_left_edge = Area { x: 0, y: 100, width: 800, height: 600 };
    assert_eq!(x11_sides(None, Sides::ALL, None), Sides::ALL);
    assert_eq!(x11_sides(None, Sides::ALL, Some((floating, SCREEN))), Sides::ALL);
    assert_eq!(x11_sides(None, Sides::ALL, Some((at_left_edge, SCREEN))), Sides { left: false, ..Sides::ALL });
    assert_eq!(x11_sides(Some(170), Sides::ALL, Some((at_left_edge, SCREEN))), Sides::ALL, "Mutter says the window is free even though it touches the edge");
    let vertical = net_wm_state(&[ATOMS.vertical], ATOMS);
    assert_eq!(x11_sides(None, vertical, Some((floating, SCREEN))), Sides { top: false, bottom: false, ..Sides::ALL });
}

#[test]
fn a_burst_of_move_events_queries_each_window_once_with_its_last_xid() {
    assert_eq!(latest([(1, 10), (2, 20), (1, 11), (1, 12)]), vec![(1, 12), (2, 20)]);
    assert!(latest(Vec::<(u8, u8)>::new()).is_empty());
}
