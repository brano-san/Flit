use super::*;

#[test]
fn the_elevated_idle_timer_starts_on_connection_and_refreshes_on_progress() {
    let mut idle = Idle::default();
    let launch = Instant::now();
    let connected = launch + IDLE_TIMEOUT * 2;
    assert!(!idle.expired(connected));
    idle.progress(connected);
    let page = connected + IDLE_TIMEOUT - Duration::from_secs(1);
    idle.progress(page);
    assert!(!idle.expired(connected + IDLE_TIMEOUT));
    assert!(idle.expired(page + IDLE_TIMEOUT));
}

#[test]
fn progress_messages_leave_room_for_a_final_result_and_reject_invalid_tags() {
    let mut messages = vec![0, 0, 0, 1];
    let result: Result<Report, Error> = Err(Error::Cancelled);
    write_frame(&mut messages, &result).unwrap();
    assert!(matches!(read_reply(&mut messages.as_slice()).unwrap(), Err(Error::Cancelled)));
    assert!(read_reply(&mut &[2u8][..]).is_err());
}
