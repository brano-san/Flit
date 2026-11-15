use super::*;
use x11rb::protocol::xproto::{MapState, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;

static POINTER_USERS: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let _pointer = POINTER_USERS.lock().unwrap_or_else(|e| e.into_inner());
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
    let paths = vec![FsPath::parse("/home/u/a b.txt").unwrap(), FsPath::parse("/home/u/c.txt").unwrap()];
    let dragging = std::thread::spawn(move || drag(paths, 1.0));
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

fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * b.width + x) * 4) as usize;
    [b.rgba[at], b.rgba[at + 1], b.rgba[at + 2], b.rgba[at + 3]]
}

fn solid(side: u32, color: [u8; 4]) -> Bitmap {
    Bitmap { width: side, height: side, rgba: color.repeat((side * side) as usize) }
}

fn badge_left(b: &Bitmap, row: u32) -> Option<u32> {
    (0..b.width).find(|&x| {
        let p = px(b, x, row);
        p[3] > 200 && p[2] > p[0] + 60
    })
}

#[test]
fn the_drag_picture_shows_the_files_own_icon_and_counts_several_files() {
    let red = solid(48, [200, 0, 0, 255]);
    let one = compose_drag(Some(&red), 1, 1.0);
    assert_eq!((one.width, one.height), (60, 60));
    assert_eq!(px(&one, 24, 12 + 24), [200, 0, 0, 255], "the file's icon, not a drawn folder");
    assert_eq!(badge_left(&one, 11), None, "one file has no counter");
    let three = compose_drag(Some(&red), 3, 1.0);
    let left = badge_left(&three, 11).expect("a counter for three files");
    assert!((3..19).any(|y| (left..three.width).any(|x| px(&three, x, y).iter().take(3).all(|&c| c > 200))), "the count is written on the badge");
    let many = compose_drag(Some(&red), 5000, 1.0);
    assert!(badge_left(&many, 11).unwrap() < left, "a long count makes a wider badge");
    assert_eq!(badge_text(5000), "999+");
    assert_eq!(badge_text(12), "12");
}

#[test]
fn without_an_icon_the_picture_falls_back_to_a_folder_and_scales_with_the_screen() {
    let folder = compose_drag(None, 1, 2.0);
    assert_eq!((folder.width, folder.height), (120, 120));
    let mid = px(&folder, 48, 24 + 60);
    assert!(mid[3] == 255 && mid[0] > 200 && mid[2] < 120, "{mid:?}");
    let broken = Bitmap { width: 4, height: 4, rgba: vec![9; 3] };
    assert_eq!(compose_drag(Some(&broken), 1, 1.0).rgba, compose_drag(None, 1, 1.0).rgba, "a malformed icon is not drawn");
}

#[test]
fn the_x11_ghost_is_shaped_to_the_picture_and_blends_its_edges_on_light_grey() {
    let mut b = Bitmap { width: 4, height: 2, rgba: vec![0; 32] };
    b.rgba[4..8].copy_from_slice(&[255, 0, 0, 255]);
    b.rgba[8..12].copy_from_slice(&[0, 0, 128, 128]);
    b.rgba[28..32].copy_from_slice(&[0, 0, 0, 16]);
    assert_eq!(opaque_runs(&b), [Rectangle { x: 1, y: 0, width: 2, height: 1 }]);
    let bgrx = over_background(&b, [200, 200, 200]);
    assert_eq!(&bgrx[4..8], &[0, 0, 255, 0]);
    assert_eq!(&bgrx[8..12], &[228, 100, 100, 0]);
    assert_eq!(&bgrx[0..4], &[200, 200, 200, 0]);
}

#[test]
fn modifiers_pick_the_action_offered_to_other_programs_like_gtk_does() {
    assert_eq!(suggested(KeyButMask::default()), Suggested::Any);
    assert_eq!(suggested(KeyButMask::CONTROL), Suggested::Copy);
    assert_eq!(suggested(KeyButMask::SHIFT), Suggested::Move);
    assert_eq!(suggested(KeyButMask::CONTROL | KeyButMask::SHIFT), Suggested::Link);
    assert_eq!(suggested(KeyButMask::MOD1), Suggested::Ask);
    assert_eq!(suggested(KeyButMask::SHIFT | KeyButMask::BUTTON1), Suggested::Move, "the held button does not matter");
    assert_eq!(offered_by_source(Suggested::Any).len(), 4, "without keys the target chooses among all actions");
    assert_eq!(offered_by_source(Suggested::Move), [Suggested::Move]);
    assert_eq!(codes_with(&[0, 0xff1b, 0x61, 0x41, 0xff1b, 0], 2, 8, 0xff1b), [8, 10]);
    assert_eq!(codes_with(&[0x61], 1, 8, 0xff1b), Vec::<u8>::new());
}

struct Seen {
    actions: Vec<Atom>,
    list: Vec<Atom>,
    left: bool,
}

fn watch_target(conn: &XCBConnection, atoms: &Atoms, window: Window) -> Seen {
    let mut seen = Seen { actions: Vec::new(), list: Vec::new(), left: false };
    let mut source = NONE;
    loop {
        match conn.wait_for_event().unwrap() {
            Event::ClientMessage(c) if c.type_ == atoms.XdndEnter => source = c.data.as_data32()[0],
            Event::ClientMessage(c) if c.type_ == atoms.XdndPosition => {
                let action = c.data.as_data32()[4];
                seen.actions.push(action);
                seen.list = conn.get_property(false, source, atoms.XdndActionList, AtomEnum::ATOM, 0, 16).unwrap().reply().unwrap().value32().unwrap().collect();
                conn.send_event(false, source, EventMask::NO_EVENT, ClientMessageEvent::new(32, source, atoms.XdndStatus, [window, 1, 0, 0, action])).unwrap();
                conn.flush().unwrap();
            }
            Event::ClientMessage(c) if c.type_ == atoms.XdndLeave => {
                seen.left = true;
                return seen;
            }
            Event::ClientMessage(c) if c.type_ == atoms.XdndDrop => {
                conn.send_event(false, source, EventMask::NO_EVENT, ClientMessageEvent::new(32, source, atoms.XdndFinished, [window, 1, *seen.actions.last().unwrap(), 0, 0])).unwrap();
                conn.flush().unwrap();
                return seen;
            }
            _ => {}
        }
    }
}

fn drag_scene(keys: &[u32], release_with_escape: bool) -> (Seen, Outcome, Atoms) {
    use x11rb::protocol::xproto::{KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
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
    let setup = input.setup();
    let map = input.get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1).unwrap().reply().unwrap();
    let code = |sym: u32| codes_with(&map.keysyms, map.keysyms_per_keycode, setup.min_keycode, sym)[0];
    let fake = |kind, detail, x, y| {
        input.xtest_fake_input(kind, detail, CURRENT_TIME, root, x, y, 0).unwrap();
        input.flush().unwrap();
        std::thread::sleep(Duration::from_millis(50));
    };
    fake(MOTION_NOTIFY_EVENT, 0, 10, 10);
    fake(BUTTON_PRESS_EVENT, 1, 0, 0);
    let target = std::thread::spawn(move || {
        let seen = watch_target(&conn, &atoms, window);
        (seen, atoms)
    });
    let paths = vec![FsPath::parse("/home/u/a.txt").unwrap()];
    let dragging = std::thread::spawn(move || drag(paths, 1.0));
    std::thread::sleep(Duration::from_millis(200));
    fake(MOTION_NOTIFY_EVENT, 0, 150, 150);
    for &k in keys {
        fake(KEY_PRESS_EVENT, code(k), 0, 0);
    }
    fake(MOTION_NOTIFY_EVENT, 0, 200, 180);
    if release_with_escape {
        fake(KEY_PRESS_EVENT, code(ESCAPE), 0, 0);
        fake(KEY_RELEASE_EVENT, code(ESCAPE), 0, 0);
    }
    fake(BUTTON_RELEASE_EVENT, 1, 0, 0);
    for &k in keys {
        fake(KEY_RELEASE_EVENT, code(k), 0, 0);
    }
    let (seen, atoms) = target.join().unwrap();
    (seen, dragging.join().unwrap().unwrap(), atoms)
}

const SHIFT_L: u32 = 0xffe1;

#[test]
fn shift_moves_and_esc_cancels_a_drag_out_of_flit() {
    let _pointer = POINTER_USERS.lock().unwrap_or_else(|e| e.into_inner());
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("no X server, skipped");
        return;
    }
    let (plain, outcome, atoms) = drag_scene(&[], false);
    assert!(matches!(outcome, Outcome::Dropped(_)));
    assert!(plain.actions.iter().all(|&a| a == atoms.XdndActionCopy), "no keys suggest a copy");
    assert_eq!(plain.list, [atoms.XdndActionCopy, atoms.XdndActionMove, atoms.XdndActionLink, atoms.XdndActionAsk]);
    let (moved, outcome, atoms) = drag_scene(&[SHIFT_L], false);
    assert!(matches!(outcome, Outcome::Dropped(_)));
    assert_eq!(moved.actions.last(), Some(&atoms.XdndActionMove), "{:?}", moved.actions);
    assert_eq!(moved.list, [atoms.XdndActionMove]);
    let (canceled, outcome, _) = drag_scene(&[], true);
    assert_eq!(outcome, Outcome::Canceled);
    assert!(canceled.left, "the target hears the drag leave");
}

#[test]
fn dropped_content_stays_copy_only_for_shift_alt_and_compositor_ask() {
    use drop_effect::{COPY, NONE};
    for (keys, proposed) in [(Some(SHIFT), Some(DndAction::Move)), (Some(ALT), Some(DndAction::Ask)), (None, Some(DndAction::Move)), (None, Some(DndAction::Ask))] {
        let (mods, ask) = incoming_choice(true, keys, proposed, Some(DndAction::Copy));
        assert_eq!(mods, Mods::default());
        assert!(!ask);
        assert_eq!(incoming_actions(true, COPY, Some(DndAction::Move)), [DndAction::Copy]);
        assert!(incoming_actions(true, NONE, Some(DndAction::Copy)).is_empty());
        assert!(incoming_actions(true, drop_effect::MOVE, Some(DndAction::Move)).is_empty());
    }
}

#[test]
fn incoming_file_paths_keep_their_move_and_right_drop_choices() {
    use drop_effect::MOVE;
    assert_eq!(incoming_choice(false, Some(SHIFT), Some(DndAction::Copy), Some(DndAction::Move)), (SHIFT, false));
    assert_eq!(incoming_choice(false, Some(ALT), None, None), (Mods::default(), true));
    assert_eq!(incoming_actions(false, MOVE, None), [DndAction::Move, DndAction::Copy, DndAction::Ask]);
}
