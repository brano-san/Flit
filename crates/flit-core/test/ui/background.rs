use super::{idle, Idle};

#[test]
fn last_window_closes_to_a_working_tray_and_the_next_launch_opens_a_window() {
    assert_eq!(idle(true, Some(true), false, false), Idle::Wait);
    assert_eq!(idle(true, Some(true), false, true), Idle::Open);
}

#[test]
fn close_without_a_tray_or_explicit_quit_ends_the_process() {
    assert_eq!(idle(false, Some(true), false, false), Idle::Exit);
    assert_eq!(idle(true, Some(false), false, false), Idle::Exit);
    assert_eq!(idle(true, Some(true), true, true), Idle::Exit);
}
