use super::*;

fn rect(id: WindowId, origin: (f64, f64), size: (u32, u32)) -> TabTarget {
    TabTarget { id, origin, size, minimized: false }
}

#[test]
fn a_dragged_tab_finds_the_other_window_and_its_local_insertion_point() {
    let windows = [rect(1, (100.0, 80.0), (600, 400)), rect(2, (-900.0, 120.0), (800, 500))];
    assert_eq!(tab_target(windows.into_iter(), 1, (-850.5, 145.25)), Some((2, 49.5, 25.25)));
}

#[test]
fn a_dragged_tab_misses_empty_space_and_its_source_window() {
    let windows = [rect(1, (100.0, 80.0), (600, 400)), rect(2, (800.0, 120.0), (800, 500))];
    for point in [(200.0, 100.0), (750.0, 145.0), (1700.0, 145.0)] {
        assert_eq!(tab_target(windows.into_iter(), 1, point), None);
    }
    assert_eq!(tab_target([].into_iter(), 1, (0.0, 0.0)), None);
}

#[test]
fn overlapping_windows_choose_the_last_activated_matching_target() {
    let lower = rect(2, (0.0, 0.0), (600, 400));
    let upper = rect(3, (100.0, 100.0), (600, 400));
    assert_eq!(tab_target([lower, upper].into_iter(), 1, (120.0, 130.0)), Some((3, 20.0, 30.0)));
    assert_eq!(tab_target([upper, lower].into_iter(), 1, (120.0, 130.0)), Some((2, 120.0, 130.0)));
    assert_eq!(tab_target([lower, upper].into_iter(), 1, (20.0, 30.0)), Some((2, 20.0, 30.0)));
}

#[test]
fn minimized_windows_do_not_receive_tabs_even_when_they_were_activated_last() {
    let lower = rect(2, (0.0, 0.0), (600, 400));
    let upper = TabTarget { minimized: true, ..rect(3, (100.0, 100.0), (600, 400)) };
    assert_eq!(tab_target([lower, upper].into_iter(), 1, (120.0, 130.0)), Some((2, 120.0, 130.0)));
    assert_eq!(tab_target([upper].into_iter(), 1, (120.0, 130.0)), None);
}

#[test]
fn window_edges_belong_only_to_the_window_starting_at_that_edge() {
    let a = rect(2, (100.0, 80.0), (600, 400));
    let b = rect(3, (700.0, 80.0), (600, 400));
    for (point, want) in [((100.0, 80.0), Some((2, 0.0, 0.0))), ((699.5, 479.5), Some((2, 599.5, 399.5))), ((700.0, 80.0), Some((3, 0.0, 0.0))), ((100.0, 480.0), None), ((99.5, 80.0), None)] {
        assert_eq!(tab_target([a, b].into_iter(), 1, point), want);
    }
}

#[test]
fn invalid_pointer_coordinates_and_empty_windows_never_receive_tabs() {
    let a = rect(2, (0.0, 0.0), (600, 400));
    for point in [(f64::NAN, 0.0), (0.0, f64::NAN), (f64::INFINITY, 0.0), (0.0, f64::NEG_INFINITY)] {
        assert_eq!(tab_target([a].into_iter(), 1, point), None);
    }
    for size in [(0, 400), (600, 0)] {
        assert_eq!(tab_target([rect(2, (0.0, 0.0), size)].into_iter(), 1, (0.0, 0.0)), None);
    }
}
#[test]
fn minimized_state_refresh_is_limited_to_once_per_quarter_second() {
    let now = Instant::now();
    assert!(tab_windows_due(None, now));
    assert!(!tab_windows_due(Some(now), now));
    assert!(!tab_windows_due(Some(now), now + Duration::from_millis(249)));
    assert!(tab_windows_due(Some(now), now + Duration::from_millis(250)));
}
#[test]
fn an_old_visibility_query_cannot_make_a_newly_minimized_window_receive_a_tab() {
    let state = MinimizedState::new(false);
    let outstanding = state.snapshot();
    state.set(true);
    state.complete(outstanding, false);
    let target = TabTarget { minimized: state.get(), ..rect(2, (0.0, 0.0), (600, 400)) };
    assert_eq!(tab_target([target].into_iter(), 1, (120.0, 130.0)), None);
}

#[test]
fn a_recent_visibility_query_can_update_the_cached_minimized_state() {
    let state = MinimizedState::new(true);
    state.complete(state.snapshot(), false);
    let target = TabTarget { minimized: state.get(), ..rect(2, (0.0, 0.0), (600, 400)) };
    assert_eq!(tab_target([target].into_iter(), 1, (120.0, 130.0)), Some((2, 120.0, 130.0)));
}
#[test]
fn a_minimize_restore_cycle_also_invalidates_an_older_visibility_query() {
    let state = MinimizedState::new(false);
    let outstanding = state.snapshot();
    state.set(true);
    state.set(false);
    state.complete(outstanding, true);
    let target = TabTarget { minimized: state.get(), ..rect(2, (0.0, 0.0), (600, 400)) };
    assert_eq!(tab_target([target].into_iter(), 1, (120.0, 130.0)), Some((2, 120.0, 130.0)));
}