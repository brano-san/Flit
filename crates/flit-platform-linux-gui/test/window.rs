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

#[test]
fn a_window_closed_during_a_minimized_check_is_released_on_the_ui_thread() {
    struct Probe(Arc<Mutex<Option<std::thread::ThreadId>>>);
    impl Drop for Probe {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = Some(std::thread::current().id());
        }
    }
    let dropped = Arc::new(Mutex::new(None));
    let window = Arc::new(Probe(dropped.clone()));
    let state = Arc::new(MinimizedState::new(false));
    let checks = vec![(Arc::downgrade(&window), state.clone(), state.snapshot())];
    let (back, returned) = mpsc::channel();
    let (inside, entered) = mpsc::channel();
    let (go, wait) = mpsc::channel::<()>();
    let worker = std::thread::spawn(move || {
        check_minimized(checks, |_| {
            inside.send(()).unwrap();
            wait.recv().unwrap();
            Some(true)
        }, &back, || {});
    });
    entered.recv().unwrap();
    drop(window);
    go.send(()).unwrap();
    worker.join().unwrap();
    assert!(dropped.lock().unwrap().is_none(), "the background check never destroys a window");
    returned.try_iter().for_each(drop);
    assert_eq!(*dropped.lock().unwrap(), Some(std::thread::current().id()));
    assert!(state.get());
    let gone: Vec<MinimizedCheck<Probe>> = vec![(Weak::new(), state.clone(), state.snapshot())];
    let (back, returned) = mpsc::channel();
    check_minimized(gone, |_| panic!("a closed window is not queried"), &back, || {});
    assert!(returned.try_iter().next().is_none());
}

#[test]
fn focus_resize_and_occlusion_reuse_the_cached_position_without_asking_x11() {
    let ask = || -> Option<(f64, f64)> { panic!("X11 position read") };
    let inset = || -> (f64, f64) { panic!("X11 inset read") };
    let first = next_target(None, 3, None, (800, 600), || (2.0, 30.0), || Some((100.0, 50.0))).unwrap();
    assert_eq!(first.0.origin, (102.0, 80.0));
    let resized = next_target(Some(first), 3, None, (900, 700), inset, ask).unwrap();
    assert_eq!((resized.0.origin, resized.0.size), ((102.0, 80.0), (900, 700)));
    let moved = next_target(Some(resized), 3, Some((10.0, 20.0)), (900, 700), inset, ask).unwrap();
    assert_eq!(moved.0.origin, (12.0, 50.0));
    let hidden = (TabTarget { minimized: true, ..moved.0 }, moved.1);
    assert!(next_target(Some(hidden), 3, None, (900, 700), inset, ask).unwrap().0.minimized);
    assert!(next_target(None, 3, None, (1, 1), || (0.0, 0.0), || None).is_none());
}

#[test]
fn kde_globals_without_an_accent_keep_the_accent_from_the_portal() {
    let blue = crate::shell::kde_accent("[General]\nAccentColor=61,174,233\n").unwrap();
    let red = crate::shell::kde_accent("[General]\nAccentColor=218,68,83\n").unwrap();
    assert_eq!(kde_accent_change("[General]\nColorScheme=BreezeDark\n", Some(blue)), None);
    assert_eq!(kde_accent_change("", None), None);
    assert_eq!(kde_accent_change("[General]\nAccentColor=61,174,233\n", Some(blue)), None, "an unchanged accent is not applied again");
    assert_eq!(kde_accent_change("[General]\nAccentColor=218,68,83\n", Some(blue)), Some(Some(red)));
    assert_eq!(kde_accent_change("[General]\nAccentColor=218,68,83\n", None), Some(Some(red)));
}
