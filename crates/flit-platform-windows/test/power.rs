use super::*;

#[test]
fn a_system_power_request_is_held_until_dropped_and_can_be_taken_again() {
    let first = Awake::hold("Flit test: copying files").expect("a power request");
    let handle = first.0;
    drop(first);
    assert!(unsafe { PowerSetRequest(handle, PowerRequestSystemRequired) }.is_err(), "the request handle is closed when the hold ends");
    let again = Awake::hold("Flit test: copying files");
    assert!(again.is_some());
}
