use super::*;

#[test]
fn a_shared_display_binds_activation_and_clipboard_but_needs_an_input_serial() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("no Wayland compositor, skipped");
        return;
    }
    let Ok(own) = Connection::connect_to_env() else { return };
    let mut wl = Wl::connect(own.backend().display_ptr().cast()).expect("the shared display answers");
    assert!(wl.manager.is_some(), "every desktop compositor has a data device manager");
    wl.pump();
    let payload = Arc::new(crate::clipboard::payload(&[flit_core::FsPath::parse("/tmp/x").unwrap()], true));
    if wl.state.serial.is_none() {
        assert!(!wl.set_selection(payload), "a selection without an input serial is refused up front");
    }
    assert!(Wl::connect(std::ptr::null_mut()).is_none());
}

#[test]
fn copying_twice_reuses_the_data_devices_made_at_connect_and_keeps_the_connection() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("no Wayland compositor, skipped");
        return;
    }
    let Ok(own) = Connection::connect_to_env() else { return };
    let mut wl = Wl::connect(own.backend().display_ptr().cast()).expect("the shared display answers");
    if wl.manager.is_none() {
        return;
    }
    let devices = wl.devices.len();
    assert_eq!(devices, wl.state.seats.len(), "one data device per seat, made once");
    wl.state.serial = Some(1);
    for _ in 0..2 {
        let payload = Arc::new(crate::clipboard::payload(&[flit_core::FsPath::parse("/tmp/x").unwrap()], false));
        assert!(wl.set_selection(payload));
        assert!(wl.queue.roundtrip(&mut wl.state).is_ok(), "the compositor keeps talking to Flit after a copy");
    }
    assert_eq!(wl.devices.len(), devices, "copying makes no new data devices");
}

#[test]
fn a_lost_display_connection_is_named_as_such_in_the_log() {
    let e = crate::window::loop_failure(winit::error::EventLoopError::ExitFailure(1));
    let text = e.to_string();
    assert!(text.contains("display server"), "{text}");
    assert!(!text.contains("Exit Failure"), "{text}");
}

#[test]
fn the_clipboard_counts_as_text_only_when_its_offer_lists_a_plain_text_type() {
    let mut offers = Offers::default();
    assert_eq!(offers.take(), None, "nothing is reported before the compositor speaks");
    offers.made(7);
    offers.offered(7, crate::clipboard::URI_LIST);
    offers.offered(7, "text/plain;charset=utf-8");
    offers.selected(Some(7));
    assert_eq!(offers.take(), Some(true));
    assert_eq!(offers.take(), None, "a change is reported once");
    offers.made(8);
    offers.offered(8, crate::clipboard::PNG);
    offers.selected(Some(8));
    assert_eq!(offers.take(), Some(false), "a picture alone is no text");
    offers.made(9);
    offers.offered(9, "UTF8_STRING");
    offers.selected(None);
    assert_eq!(offers.take(), Some(false), "an emptied clipboard has no text");
}

#[test]
fn a_dragged_offer_never_changes_what_the_clipboard_holds() {
    let mut offers = Offers::default();
    offers.made(3);
    offers.offered(3, "text/plain");
    offers.forget(3);
    assert_eq!(offers.take(), None);
    offers.offered(3, "text/plain");
    offers.selected(Some(3));
    assert_eq!(offers.take(), Some(false), "types of a forgotten offer are not remembered");
}
