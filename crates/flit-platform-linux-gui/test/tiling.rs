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
    assert_eq!(against_workarea(quick_tiled_left, SCREEN), Sides { top: false, bottom: false, ..Sides::ALL });
    let floating = Area { x: 100, y: 100, width: 800, height: 600 };
    assert_eq!(against_workarea(floating, SCREEN), Sides::ALL);
    let top_right_quarter = Area { x: 960, y: 32, width: 960, height: 524 };
    assert_eq!(against_workarea(top_right_quarter, SCREEN), Sides::ALL);
}

#[test]
fn x11_sides_trusts_mutter_constraints_over_geometry_and_adds_the_wm_state() {
    let floating = Area { x: 100, y: 100, width: 800, height: 600 };
    let at_left_edge = Area { x: 0, y: 100, width: 800, height: 600 };
    assert_eq!(x11_sides(None, Sides::ALL, None), Sides::ALL);
    assert_eq!(x11_sides(None, Sides::ALL, Some((floating, SCREEN))), Sides::ALL);
    assert_eq!(x11_sides(None, Sides::ALL, Some((at_left_edge, SCREEN))), Sides::ALL);
    assert_eq!(x11_sides(Some(170), Sides::ALL, Some((at_left_edge, SCREEN))), Sides::ALL, "Mutter says the window is free even though it touches the edge");
    assert_eq!(x11_sides(Some(LEFT_TILED), Sides::ALL, Some((floating, SCREEN))), Sides { left: false, ..Sides::ALL });
    let vertical = net_wm_state(&[ATOMS.vertical], ATOMS);
    assert_eq!(x11_sides(None, vertical, Some((floating, SCREEN))), Sides { top: false, bottom: false, ..Sides::ALL });
}

#[test]
fn a_floating_window_in_the_top_left_corner_keeps_all_resize_edges() {
    let corner = Area { width: 800, height: 600, ..SCREEN };
    assert_eq!(x11_sides(None, Sides::ALL, Some((corner, SCREEN))), Sides::ALL);
}

#[test]
fn a_full_height_window_at_the_left_edge_can_still_resize_horizontally() {
    let full_height = Area { width: 800, ..SCREEN };
    assert_eq!(x11_sides(None, Sides::ALL, Some((full_height, SCREEN))), Sides { top: false, bottom: false, ..Sides::ALL });
    let full_width = Area { height: 600, ..SCREEN };
    assert_eq!(x11_sides(None, Sides::ALL, Some((full_width, SCREEN))), Sides { left: false, right: false, ..Sides::ALL });
}

#[test]
fn maximized_and_fullscreen_windows_keep_their_edges_locked() {
    let window = Area { x: 100, y: 100, width: 800, height: 600 };
    for states in [&[ATOMS.vertical, ATOMS.horizontal][..], &[ATOMS.fullscreen]] {
        assert_eq!(x11_sides(None, net_wm_state(states, ATOMS), Some((window, SCREEN))), Sides::NONE);
    }
}

fn window_property(window: u32) -> GetPropertyReply {
    GetPropertyReply { format: 32, type_: AtomEnum::WINDOW.into(), value_len: 1, value: window.to_ne_bytes().to_vec(), ..Default::default() }
}

#[test]
fn only_a_self_referencing_wm_window_confirms_an_ewmh_manager() {
    assert_eq!(supporting_wm(1, |_| Some(GetPropertyReply::default())), None);
    assert_eq!(supporting_wm(1, |window| (window == 1).then(|| window_property(2))), None);
    assert_eq!(supporting_wm(1, |_| Some(window_property(3))), Some(3));
    assert_eq!(supporting_wm(1, |window| Some(window_property(window + 1))), None);
    assert_eq!(supporting_wm(1, |_| Some(window_property(1))), None);
    assert_eq!(supporting_wm(1, |_| Some(window_property(0))), None);
    let wrong_type = GetPropertyReply { type_: AtomEnum::CARDINAL.into(), ..window_property(3) };
    assert_eq!(supporting_wm(1, |_| Some(wrong_type.clone())), None);
    let multiple = GetPropertyReply { bytes_after: 4, ..window_property(3) };
    assert_eq!(supporting_wm(1, |_| Some(multiple.clone())), None);
}

#[test]
fn x11_tiling_managers_keep_flits_title_bar() {
    for name in ["I3 4.21", "dwm-6.5", "BSPWM", "awesome v4", "xmonad", "herbstluftwm", "qtile", "lG3d", "Sway"] {
        assert!(!wm_draws_frames(name, false), "{name}");
    }
    assert!(!wm_draws_frames("wlroots wm", true));
    for name in ["KWin", "Mutter", "Xfwm4", "Openbox", "wlroots wm", "éü"] {
        assert!(wm_draws_frames(name, false), "{name}");
    }
}

#[test]
fn property_notifications_refresh_edges_without_a_move() {
    use std::cell::Cell;
    let mut watched = WatchedWindows { ids: HashMap::from([(10, 1), (20, 2)]), ..Default::default() };
    let edges = std::cell::RefCell::new(HashMap::from([(1, Sides::ALL), (2, Sides::ALL)]));
    let reads = Cell::new(0);
    let read = |window| {
        reads.set(reads.get() + 1);
        let state = if window == 10 { &[ATOMS.vertical][..] } else { &[] };
        Some(net_wm_state(state, ATOMS))
    };
    let report = |id, sides| { edges.borrow_mut().insert(id, sides); };
    let property = |window, atom| PropertyNotifyEvent { window, atom, ..Default::default() };
    let (root, state, gtk, workarea, desktop) = (1, 101, 102, 103, 104);
    for (window, atom) in [(10, state), (10, state), (10, workarea), (1, state), (30, state)] {
        watched.property_changed(property(window, atom), root, [state, gtk], [workarea, desktop]);
    }
    watched.refresh(read, report);
    assert_eq!(reads.get(), 1);
    assert_eq!(edges.borrow()[&1], Sides { top: false, bottom: false, ..Sides::ALL });
    watched.property_changed(property(10, gtk), root, [state, gtk], [workarea, desktop]);
    watched.refresh(read, report);
    assert_eq!(reads.get(), 2);
    watched.property_changed(property(root, workarea), root, [state, gtk], [workarea, desktop]);
    watched.refresh(read, report);
    assert_eq!(reads.get(), 4);
    assert_eq!(edges.borrow()[&2], Sides::ALL);
}
