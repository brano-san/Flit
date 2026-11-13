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
