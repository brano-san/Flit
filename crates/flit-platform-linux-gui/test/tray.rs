use super::*;
use flit_core::{FsPath, Location};

#[test]
fn exported_bookmark_commands_launch_only_on_click() {
    let loc = Location::from_local(FsPath::parse("/tmp/bookmark").unwrap());
    let menu = Menu { new_window: "New window".into(), bookmarks: "Bookmarks".into(), pinned: vec![(loc.clone(), "A_B".into())], quit: "Quit".into() };
    let tree = Item::tree(Some(&menu));
    assert_eq!(tree.clicked(4, "clicked"), Some(Action::Bookmark(loc)));
    assert_eq!(tree.clicked(4, "hovered"), None);
    assert_eq!(tree.clicked(3, "clicked"), None);
    assert_eq!(tree.clicked(17, "clicked"), None);
}

#[test]
fn tray_responds_with_the_wire_signatures_for_layout_and_properties() {
    let api = crate::dbus::load().expect("libdbus-1 is required for serializing tray replies");
    let menu = Menu { new_window: "New window".into(), bookmarks: "Bookmarks".into(), pinned: Vec::new(), quit: "Quit".into() };
    let tree = Item::tree(Some(&menu));
    let export = Export { tree: &tree, revision: 8, active: true };
    let props = c"org.freedesktop.DBus.Properties";
    let layout = export.reply(MENU_PATH, MENU, c"GetLayout", &[Arg::I(0), Arg::I(-1), Arg::L(Vec::new())], |_| unreachable!()).unwrap();
    assert!(matches!(layout.as_slice(), [V::U(8), V::Tuple(root)] if matches!(root.as_slice(), [V::I(0), _, V::Array("v", children)] if children.len() == 2)));
    let args = |value: &CStr| Arg::S(value.to_str().unwrap().into());
    let check = |reply: Vec<V>, expected: &CStr| unsafe {
        let msg = (api.new_signal)(MENU_PATH.as_ptr(), MENU.as_ptr(), c"TestReply".as_ptr());
        assert!(!msg.is_null());
        let mut it = crate::dbus::Iter::new();
        (api.iter_init_append)(msg, &mut it);
        for value in &reply {
            crate::atspi::put(api, &mut it, "", value);
        }
        let actual = CStr::from_ptr((api.signature)(msg)).to_owned();
        (api.unref)(msg);
        assert_eq!(actual.as_c_str(), expected);
    };
    check(layout, c"u(ia{sv}av)");
    for (path, iface, key, signature) in [
        (ITEM_PATH, ITEM, c"ToolTip", c"(sa(iiay)ss)"),
        (ITEM_PATH, ITEM, c"Menu", c"o"),
        (MENU_PATH, MENU, c"Version", c"u"),
        (MENU_PATH, MENU, c"IconThemePath", c"as"),
    ] {
        let get = export.reply(path, props, c"Get", &[args(iface), args(key)], |_| unreachable!()).unwrap();
        let [V::Var(value)] = get.as_slice() else { panic!("Get must return a variant") };
        check(vec![(**value).clone()], signature);
        check(get, c"v");
        check(export.reply(path, props, c"GetAll", &[args(iface)], |_| unreachable!()).unwrap(), c"a{sv}");
    }
}

#[test]
fn tray_answers_calls_to_unknown_objects_and_interfaces() {
    let tree = Item::tree(None);
    let export = Export { tree: &tree, revision: 0, active: true };
    assert_eq!(export.reply(c"/Unrelated", ITEM, c"Activate", &[], |_| unreachable!()), Err(c"org.freedesktop.DBus.Error.UnknownObject"));
    assert_eq!(export.reply(ITEM_PATH, c"org.example.Unrelated", c"Activate", &[], |_| unreachable!()), Err(c"org.freedesktop.DBus.Error.UnknownMethod"));
    assert_eq!(export.reply(MENU_PATH, ITEM, c"Activate", &[Arg::I(0), Arg::I(0)], |_| unreachable!()), Err(c"org.freedesktop.DBus.Error.UnknownMethod"));
}

#[test]
fn tray_waits_for_a_restarted_host_and_reports_absence_only_after_the_deadline() {
    let mut availability = HostAvailability::default();
    let now = Instant::now();
    assert_eq!(availability.update(true, now), (Some(true), None));
    assert_eq!(availability.update(false, now), (None, Some(HOST_GRACE)));
    assert_eq!(availability.update(false, now + Duration::from_secs(1)), (None, Some(HOST_GRACE - Duration::from_secs(1))));
    assert_eq!(availability.update(true, now + Duration::from_secs(2)), (Some(true), None));
    let lost_again = now + Duration::from_secs(3);
    assert_eq!(availability.update(false, lost_again), (None, Some(HOST_GRACE)));
    assert_eq!(availability.update(false, lost_again + HOST_GRACE), (Some(false), None));
    assert_eq!(availability.update(true, lost_again + HOST_GRACE), (Some(true), None));
}

#[test]
fn a_restarted_watcher_registers_the_item_again() {
    let mut tray = Registration::default();
    let old = CString::new(":1.2").unwrap();
    let new = CString::new(":1.3").unwrap();
    tray.refresh(Some(old.clone()), false, |_| unreachable!());
    assert!(!tray.registered);
    tray.refresh(Some(old.clone()), true, |dest| {
        assert_eq!(dest, old.as_c_str());
        true
    });
    assert!(tray.registered);
    tray.refresh(Some(old), true, |_| panic!("the same host was already registered"));
    tray.refresh(None, false, |_| unreachable!());
    assert!(!tray.registered);
    tray.refresh(Some(new.clone()), true, |dest| {
        assert_eq!(dest, new.as_c_str());
        true
    });
    assert!(tray.registered);
    tray.refresh(Some(new.clone()), false, |_| unreachable!());
    assert!(!tray.registered);
    tray.refresh(Some(new), true, |_| true);
    assert!(tray.registered);
}

#[test]
fn tray_worker_starts_on_enable_wakes_on_change_and_stops_on_disable() {
    let tray = Tray::new(Arc::new(|| {}));
    let (tx, started) = std::sync::mpsc::channel();
    let listener = listen(tray.clone(), move |_| {
        let wakeup = Arc::new(Wakeup::new().unwrap());
        tx.send(wakeup.clone()).unwrap();
        Some(wakeup)
    });
    assert!(started.try_recv().is_err());
    let mut menu = Menu { new_window: "New window".into(), bookmarks: "Bookmarks".into(), pinned: Vec::new(), quit: "Quit".into() };
    tray.set_menu(Some(menu.clone()));
    let woken = started.try_recv().unwrap();
    let pending = || {
        let mut fd = woken.poll_fd();
        unsafe { libc::poll(&mut fd, 1, 0) == 1 }
    };
    assert!(!pending());
    tray.set_menu(Some(menu.clone()));
    assert!(!pending());
    menu.quit = "Exit".into();
    tray.set_menu(Some(menu.clone()));
    assert!(pending());
    assert!(started.try_recv().is_err());
    woken.drain();
    assert!(!pending());
    woken.publish(&tray, Some(true));
    tray.set_menu(None);
    assert_eq!(tray.available(), None);
    assert!(pending());
    assert!(!woken.running());
    woken.publish(&tray, Some(false));
    assert_eq!(tray.available(), None);
    tray.set_menu(Some(menu));
    let next = started.try_recv().unwrap();
    assert!(next.running());
    drop(listener);
    assert!(!next.running());
}

