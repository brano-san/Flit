use super::*;
use flit_core::ui::{MenuItem, MenuKind};

fn context_reply(kind: MenuKind, label: S) -> Event {
    Event::ShowMenu {
        items: vec![MenuItem::command(label.t(), String::new())],
        x: 0.0,
        y: 0.0,
        launches: Vec::new(),
        kind,
        shell: Vec::new(),
        pending: false,
    }
}

#[test]
fn a_late_file_menu_cannot_replace_the_latest_folder_menu() {
    let replies = MenuReplies::default();
    let file = replies.begin();
    let folder = replies.begin();
    folder.send(context_reply(MenuKind::Folder, S::MenuOpenInNewTab)).unwrap();
    let _ = file.send(context_reply(MenuKind::File, S::MenuOpenWith));
    assert!(matches!(replies.ready(), Some(Event::ShowMenu { kind: MenuKind::Folder, items, .. })
        if items == [MenuItem::command(S::MenuOpenInNewTab.t(), String::new())]));
    assert!(replies.ready().is_none(), "the late file reply must not replace the folder menu");
}

#[test]
fn a_new_background_click_discards_a_queued_item_menu_without_affecting_other_windows() {
    let first_window = MenuReplies::default();
    let second_window = MenuReplies::default();
    let file = first_window.begin();
    let other = second_window.begin();
    file.send(context_reply(MenuKind::File, S::MenuOpenWith)).unwrap();
    let background = first_window.begin();
    other.send(context_reply(MenuKind::Folder, S::MenuOpenInNewTab)).unwrap();
    assert!(first_window.ready().is_none(), "the old file reply must not open while the background is prepared");
    background.send(context_reply(MenuKind::Background, S::MenuPaste)).unwrap();
    assert!(matches!(first_window.ready(), Some(Event::ShowMenu { kind: MenuKind::Background, items, .. })
        if items == [MenuItem::command(S::MenuPaste.t(), String::new())]));
    assert!(matches!(second_window.ready(), Some(Event::ShowMenu { kind: MenuKind::Folder, .. })));
}

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
    let first = next_target(None, 3, TargetGeometry::Cached, (800, 600), || (2.0, 30.0), || Some((100.0, 50.0))).unwrap();
    assert_eq!(first.0.origin, (102.0, 80.0));
    let resized = next_target(Some(first), 3, TargetGeometry::Cached, (900, 700), inset, ask).unwrap();
    assert_eq!((resized.0.origin, resized.0.size), ((102.0, 80.0), (900, 700)));
    let moved = next_target(Some(resized), 3, TargetGeometry::Moved(10.0, 20.0), (900, 700), || (1.0, 20.0), ask).unwrap();
    assert_eq!(moved.0.origin, (11.0, 40.0));
    let hidden = (TabTarget { minimized: true, ..moved.0 }, moved.1);
    assert!(next_target(Some(hidden), 3, TargetGeometry::Cached, (900, 700), inset, ask).unwrap().0.minimized);
    assert!(next_target(None, 3, TargetGeometry::Cached, (1, 1), || (0.0, 0.0), || None).is_none());
}

#[test]
fn changing_decorations_refreshes_the_tab_target_before_the_next_move() {
    let before = (TabTarget { id: 3, origin: (100.0, 50.0), size: (800, 600), minimized: false }, (0.0, 0.0));
    let after = next_target(Some(before), 3, TargetGeometry::Refresh, (800, 600), || (3.0, 32.0), || Some((100.0, 50.0))).unwrap();
    assert_eq!(after.0.origin, (103.0, 82.0));
    let hidden = (TabTarget { minimized: true, ..after.0 }, after.1);
    let undecorated = next_target(Some(hidden), 3, TargetGeometry::Refresh, (800, 600), || (0.0, 0.0), || Some((90.0, 40.0))).unwrap();
    assert_eq!(undecorated.0.origin, (90.0, 40.0));
    assert!(undecorated.0.minimized);
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

fn pointer(state: ElementState, button: ButtonSource) -> WindowEvent {
    WindowEvent::PointerButton { device_id: None, primary: true, state, position: (5.0, 5.0).into(), button, is_macos_activation_click: false }
}

fn after(replies: &MenuReplies, event: &WindowEvent) {
    if ends_menu_wait(event) {
        replies.cancel();
    }
}

#[test]
fn a_file_menu_that_finishes_after_another_click_or_leaving_the_window_never_opens() {
    for interrupt in [pointer(ElementState::Pressed, ButtonSource::Mouse(winit::event::MouseButton::Left)), pointer(ElementState::Pressed, ButtonSource::Mouse(winit::event::MouseButton::Right)), WindowEvent::Focused(false)] {
        let replies = MenuReplies::default();
        let file = replies.begin();
        after(&replies, &interrupt);
        let _ = file.send(context_reply(MenuKind::File, S::MenuOpenWith));
        assert!(replies.ready().is_none(), "{interrupt:?} ends the wait, so the old file menu cannot pop up over what came next");
    }
}

#[test]
fn releasing_the_button_moving_or_regaining_focus_keeps_the_menu_that_is_still_being_built() {
    let replies = MenuReplies::default();
    let file = replies.begin();
    for event in [pointer(ElementState::Released, ButtonSource::Mouse(winit::event::MouseButton::Right)), WindowEvent::Focused(true), WindowEvent::PointerLeft { device_id: None, position: None, primary: true, kind: PointerKind::Mouse }] {
        after(&replies, &event);
    }
    file.send(context_reply(MenuKind::File, S::MenuOpenWith)).unwrap();
    assert!(matches!(replies.ready(), Some(Event::ShowMenu { kind: MenuKind::File, .. })));
    replies.cancel();
    let folder = replies.begin();
    folder.send(context_reply(MenuKind::Folder, S::MenuOpenInNewTab)).unwrap();
    assert!(matches!(replies.ready(), Some(Event::ShowMenu { kind: MenuKind::Folder, .. })), "a click after a cancelled wait gets its own menu");
}

#[test]
fn x11_waits_for_the_wm_probe_and_wayland_needs_the_decoration_manager() {
    assert_eq!(server_frames(false, None, None), None);
    assert_eq!(server_frames(false, None, Some(false)), Some(false));
    assert_eq!(server_frames(false, None, Some(true)), Some(true));
    assert_eq!(server_frames(true, Some(true), None), Some(true));
    assert_eq!(server_frames(true, Some(false), None), Some(false));
    assert_eq!(server_frames(true, None, None), Some(false));
}

#[test]
fn a_new_x11_window_starts_with_native_frames_when_the_probe_already_confirmed_them() {
    assert!(initial_decorations(false, true, Some(true)));
    assert!(!initial_decorations(false, true, None));
    assert!(!initial_decorations(false, true, Some(false)));
    assert!(!initial_decorations(false, false, Some(true)));
    assert!(!initial_decorations(true, true, Some(true)));
}
