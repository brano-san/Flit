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

const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };
const ALT: Mods = Mods { ctrl: false, shift: false, alt: true };

#[test]
fn an_outside_drag_on_wayland_follows_the_action_the_compositor_chose_against_flits_preference() {
    let none = Mods::default();
    assert_eq!(drop_choice(None, None, None), (none, false), "no proposal leaves Flit's own rule");
    assert_eq!(drop_choice(None, Some(DndAction::Move), Some(DndAction::Move)), (none, false), "the compositor took Flit's preference: nobody forced it");
    assert_eq!(drop_choice(None, Some(DndAction::Copy), Some(DndAction::Move)), (CTRL, false));
    assert_eq!(drop_choice(None, Some(DndAction::Move), Some(DndAction::Copy)), (SHIFT, false));
    assert_eq!(drop_choice(None, Some(DndAction::Ask), Some(DndAction::Move)), (none, true));
    assert!(chose_freely(None, Some(DndAction::Move), Some(DndAction::Move)));
    assert!(!chose_freely(None, Some(DndAction::Copy), Some(DndAction::Move)));
}

#[test]
fn held_keys_win_over_the_proposal_and_alt_asks_instead_of_being_refused() {
    let both = Mods { ctrl: true, shift: true, alt: false };
    assert_eq!(drop_choice(Some(both), Some(DndAction::Move), Some(DndAction::Copy)), (both, false), "Ctrl+Shift stays a link");
    assert_eq!(drop_choice(Some(ALT), None, None), (Mods::default(), true));
    assert_eq!(drop_choice(Some(SHIFT), Some(DndAction::Copy), Some(DndAction::Move)), (SHIFT, false));
    assert!(chose_freely(Some(SHIFT), Some(DndAction::Copy), Some(DndAction::Move)));
}

#[test]
fn flit_offers_its_preferred_action_first_and_refuses_only_when_nothing_fits() {
    use drop_effect::*;
    assert_eq!(offered_actions(NONE, Some(DndAction::Move)), Vec::<DndAction>::new());
    assert_eq!(offered_actions(MOVE, None), [DndAction::Move, DndAction::Copy, DndAction::Ask]);
    assert_eq!(offered_actions(LINK, None), [DndAction::Copy, DndAction::Move, DndAction::Ask], "a link travels as a copy, Flit makes it itself");
    assert_eq!(offered_actions(COPY, Some(DndAction::Move)), [DndAction::Move, DndAction::Copy, DndAction::Ask], "a forced copy keeps Flit's own preference first");
    assert_eq!(wire_action(COPY | MOVE), Some(DndAction::Move));
    assert_eq!(wire_action(NONE), None);
}

fn compositor(held: Option<DndAction>, offered: &[DndAction]) -> Option<DndAction> {
    held.filter(|h| offered.contains(h)).or(offered.first().copied())
}

fn hover(held: Option<DndAction>, preferred: &mut Option<DndAction>, offered: &mut Vec<DndAction>) -> (u32, bool) {
    let proposed = compositor(held, offered);
    let (mods, ask) = drop_choice(None, proposed, *preferred);
    let effect = flit_core::ui::choose_effect(drop_effect::COPY | drop_effect::MOVE | drop_effect::LINK, mods, true);
    if chose_freely(None, proposed, *preferred) {
        *preferred = wire_action(effect);
    }
    *offered = offered_actions(effect, *preferred);
    (effect, ask)
}

#[test]
fn keys_held_over_flit_during_a_drag_from_another_program_switch_copy_and_move_without_flicker() {
    use drop_effect::*;
    let mut offered = offered_actions(COPY, None);
    let mut preferred = offered.first().copied();
    let mut run = |held: Option<DndAction>| (0..6).map(|_| hover(held, &mut preferred, &mut offered)).collect::<Vec<_>>();
    let settled = |steps: Vec<(u32, bool)>| {
        let last = *steps.last().unwrap();
        assert!(steps[steps.len() - 4..].iter().all(|&s| s == last), "{steps:?}");
        last
    };
    assert_eq!(settled(run(None)), (MOVE, false), "same volume moves by default");
    assert_eq!(settled(run(Some(DndAction::Copy))), (COPY, false), "Ctrl copies");
    assert_eq!(settled(run(None)), (MOVE, false), "letting go of Ctrl moves again");
    assert!(settled(run(Some(DndAction::Ask))).1, "Alt asks");
    assert_eq!(settled(run(Some(DndAction::Move))), (MOVE, false));
    assert_eq!(settled(run(None)), (MOVE, false));
}

#[test]
fn ctrl_held_before_the_drag_reaches_flit_still_copies() {
    let mut offered = offered_actions(drop_effect::COPY, None);
    let mut preferred = offered.first().copied();
    let steps: Vec<(u32, bool)> = (0..6).map(|_| hover(Some(DndAction::Copy), &mut preferred, &mut offered)).collect();
    assert!(steps[2..].iter().all(|&s| s == (drop_effect::COPY, false)), "{steps:?}");
}
