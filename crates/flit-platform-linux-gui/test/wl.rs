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
    if wl.state.input.is_none() {
        assert!(!wl.set_selection(payload), "a selection without an input serial is refused up front");
    }
    assert!(Wl::connect(std::ptr::null_mut()).is_none());
}

#[test]
fn copying_twice_releases_the_clipboard_devices_and_keeps_the_connection() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("no Wayland compositor, skipped");
        return;
    }
    let Ok(own) = Connection::connect_to_env() else { return };
    let mut wl = Wl::connect(own.backend().display_ptr().cast()).expect("the shared display answers");
    if wl.manager.is_none() {
        return;
    }
    let Some((seat, ..)) = wl.state.seats.first() else { return };
    wl.state.input = Some((seat.clone(), 1));
    for _ in 0..2 {
        let payload = Arc::new(crate::clipboard::payload(&[flit_core::FsPath::parse("/tmp/x").unwrap()], false));
        assert!(wl.set_selection(payload));
        assert!(wl.queue.roundtrip(&mut wl.state).is_ok(), "the compositor keeps talking to Flit after a copy");
    }
}

#[test]
fn a_lost_display_connection_is_named_as_such_in_the_log() {
    let e = crate::window::loop_failure(winit::error::EventLoopError::ExitFailure(1));
    let text = e.to_string();
    assert!(text.contains("display server"), "{text}");
    assert!(!text.contains("Exit Failure"), "{text}");
}

#[test]
fn only_the_xdg_decoration_global_means_the_compositor_can_draw_frames() {
    let kwin = ["wl_compositor", "xdg_wm_base", "zxdg_decoration_manager_v1", "org_kde_kwin_server_decoration_manager"];
    let mutter = ["wl_compositor", "xdg_wm_base", "gtk_shell1", "xdg_activation_v1"];
    assert!(offers_server_decorations(kwin));
    assert!(!offers_server_decorations(mutter));
    assert!(!offers_server_decorations(["org_kde_kwin_server_decoration_manager"]), "winit asks only through xdg-decoration");
}
