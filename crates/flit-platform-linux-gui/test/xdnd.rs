use super::*;
use x11rb::protocol::xproto::{MapState, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::wrapper::ConnectionExt as _;

struct Dropped {
    data: Vec<u8>,
    entered: bool,
}

fn receive(conn: &XCBConnection, atoms: &Atoms, window: Window) -> Dropped {
    let mut out = Dropped { data: Vec::new(), entered: false };
    let mut source = NONE;
    loop {
        match conn.wait_for_event().unwrap() {
            Event::ClientMessage(c) if c.type_ == atoms.XdndEnter => {
                let d = c.data.as_data32();
                out.entered = d[1] >> 24 == VERSION && d[2] == atoms.UriList;
                source = d[0];
            }
            Event::ClientMessage(c) if c.type_ == atoms.XdndPosition => {
                conn.send_event(false, source, EventMask::NO_EVENT, ClientMessageEvent::new(32, source, atoms.XdndStatus, [window, 1, 0, 0, atoms.XdndActionCopy])).unwrap();
                conn.flush().unwrap();
            }
            Event::ClientMessage(c) if c.type_ == atoms.XdndDrop => {
                conn.convert_selection(window, atoms.XdndSelection, atoms.UriList, atoms.UriList, c.data.as_data32()[2]).unwrap();
                conn.flush().unwrap();
            }
            Event::SelectionNotify(n) => {
                out.data = conn.get_property(true, window, n.property, AtomEnum::ANY, 0, u32::MAX).unwrap().reply().unwrap().value;
                conn.send_event(false, source, EventMask::NO_EVENT, ClientMessageEvent::new(32, source, atoms.XdndFinished, [window, 1, atoms.XdndActionCopy, 0, 0])).unwrap();
                conn.flush().unwrap();
                return out;
            }
            _ => {}
        }
    }
}

#[test]
fn files_dragged_out_reach_an_xdnd_target_as_a_uri_list() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("no X server, skipped");
        return;
    }
    let (conn, screen) = XCBConnection::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let atoms = Atoms::new(&conn).unwrap().reply().unwrap();
    let window = conn.generate_id().unwrap();
    conn.create_window(0, window, root, 100, 100, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new().override_redirect(1)).unwrap();
    conn.change_property32(PropMode::REPLACE, window, atoms.XdndAware, AtomEnum::ATOM, &[VERSION]).unwrap();
    conn.map_window(window).unwrap();
    conn.flush().unwrap();
    while conn.get_window_attributes(window).unwrap().reply().unwrap().map_state != MapState::VIEWABLE {
        std::thread::sleep(Duration::from_millis(5));
    }
    let (input, _) = XCBConnection::connect(None).unwrap();
    let fake = |kind, detail, x, y| {
        input.xtest_fake_input(kind, detail, CURRENT_TIME, root, x, y, 0).unwrap();
        input.flush().unwrap();
        std::thread::sleep(Duration::from_millis(50));
    };
    fake(MOTION_NOTIFY_EVENT, 0, 10, 10);
    fake(BUTTON_PRESS_EVENT, 1, 0, 0);
    let target = std::thread::spawn(move || receive(&conn, &atoms, window));
    let list = "file:///home/u/a%20b.txt\r\nfile:///home/u/c.txt\r\n";
    let dragging = std::thread::spawn(move || drag(list.to_owned()));
    std::thread::sleep(Duration::from_millis(200));
    fake(MOTION_NOTIFY_EVENT, 0, 150, 150);
    fake(MOTION_NOTIFY_EVENT, 0, 200, 180);
    fake(BUTTON_RELEASE_EVENT, 1, 0, 0);
    let got = target.join().unwrap();
    assert!(got.entered);
    assert_eq!(String::from_utf8(got.data).unwrap(), list);
    assert_eq!(dragging.join().unwrap().unwrap(), Outcome::Dropped(window));
}
