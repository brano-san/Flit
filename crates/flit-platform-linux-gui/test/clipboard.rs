use super::*;
use x11rb::protocol::xproto::Window;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn get<'a>(payload: &'a Payload, mime: &str) -> &'a [u8] {
    &payload.iter().find(|(t, _)| t == mime).unwrap().1
}

#[test]
fn copied_files_are_offered_in_every_file_manager_format() {
    let paths = [p("/home/u/a b.txt"), p("/home/u/ж")];
    let copy = payload(&paths, false);
    assert_eq!(copy.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), [GNOME, KDE_CUT, URI_LIST, TEXT, PLAIN]);
    assert_eq!(get(&copy, GNOME), b"copy\nfile:///home/u/a%20b.txt\nfile:///home/u/%D0%B6");
    assert_eq!(get(&copy, KDE_CUT), b"0");
    assert_eq!(get(&copy, URI_LIST), b"file:///home/u/a%20b.txt\r\nfile:///home/u/%D0%B6\r\n");
    assert_eq!(get(&copy, TEXT), "/home/u/a b.txt\n/home/u/ж".as_bytes());
    assert_eq!(get(&copy, PLAIN), get(&copy, TEXT));
    let cut = payload(&paths[..1], true);
    assert_eq!(get(&cut, GNOME), b"cut\nfile:///home/u/a%20b.txt");
    assert_eq!(get(&cut, KDE_CUT), b"1");
}

fn typed(list: &[(&str, &[u8])]) -> Vec<(String, Vec<u8>)> {
    list.iter().map(|(t, b)| (t.to_string(), b.to_vec())).collect()
}

#[test]
fn files_from_other_file_managers_keep_their_cut_marker() {
    let nautilus = typed(&[(GNOME, b"cut\nfile:///tmp/a\nfile:///tmp/b"), (URI_LIST, b"file:///tmp/a\r\nfile:///tmp/b\r\n")]);
    assert_eq!(files(&nautilus), Some((vec![p("/tmp/a"), p("/tmp/b")], true)));
    let dolphin = typed(&[(URI_LIST, b"file:///tmp/c%20d\r\n"), (KDE_CUT, b"1")]);
    assert_eq!(files(&dolphin), Some((vec![p("/tmp/c d")], true)));
    let dolphin_copy = typed(&[(URI_LIST, b"file:///tmp/c\r\n"), (KDE_CUT, b"0")]);
    assert_eq!(files(&dolphin_copy), Some((vec![p("/tmp/c")], false)));
    let copied = typed(&[(GNOME, b"copy\nfile:///tmp/e\n")]);
    assert_eq!(files(&copied), Some((vec![p("/tmp/e")], false)));
    let empty_gnome = typed(&[(GNOME, b""), (URI_LIST, b"# comment\r\nfile:///tmp/f\r\n")]);
    assert_eq!(files(&empty_gnome), Some((vec![p("/tmp/f")], false)));
    assert_eq!(files(&typed(&[(TEXT, b"/tmp/x")])), None);
    let ours = payload(&[p("/srv/x")], true);
    assert_eq!(files(&ours), Some((vec![p("/srv/x")], true)));
}

fn fetch(conn: &XCBConnection, window: Window, clipboard: Atom, target: Atom) -> Option<Vec<u8>> {
    conn.convert_selection(window, clipboard, target, target, CURRENT_TIME).unwrap();
    conn.flush().unwrap();
    loop {
        if let Event::SelectionNotify(n) = conn.wait_for_event().unwrap() {
            if n.property == NONE {
                return None;
            }
            return Some(conn.get_property(true, window, n.property, AtomEnum::ANY, 0, u32::MAX).unwrap().reply().unwrap().value);
        }
    }
}

#[test]
fn an_x11_clipboard_owner_answers_every_format_until_it_is_replaced() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("no X server, skipped");
        return;
    }
    let Ok((conn, screen)) = XCBConnection::connect(None) else { return };
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id().unwrap();
    conn.create_window(0, window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new()).unwrap();
    own_selection("FLIT_TEST_CLIPBOARD", payload(&[p("/tmp/flit clip")], true)).unwrap();
    let atoms = intern(&conn, &["FLIT_TEST_CLIPBOARD", "TARGETS", GNOME, KDE_CUT, URI_LIST, "UTF8_STRING", "image/png"]).unwrap();
    let (clipboard, targets) = (atoms[0], atoms[1]);
    let listed: Vec<Atom> = fetch(&conn, window, clipboard, targets).unwrap().chunks(4).map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect();
    assert!(atoms[2..6].iter().all(|a| listed.contains(a)), "{listed:?}");
    assert_eq!(fetch(&conn, window, clipboard, atoms[2]).unwrap(), b"cut\nfile:///tmp/flit%20clip");
    assert_eq!(fetch(&conn, window, clipboard, atoms[3]).unwrap(), b"1");
    assert_eq!(fetch(&conn, window, clipboard, atoms[4]).unwrap(), b"file:///tmp/flit%20clip\r\n");
    assert_eq!(fetch(&conn, window, clipboard, atoms[5]).unwrap(), b"/tmp/flit clip");
    assert_eq!(fetch(&conn, window, clipboard, atoms[6]), None);
    own_selection("FLIT_TEST_CLIPBOARD", payload(&[p("/tmp/other")], false)).unwrap();
    assert_eq!(fetch(&conn, window, clipboard, atoms[2]).unwrap(), b"copy\nfile:///tmp/other");
    let names = x11_names();
    assert!(names.iter().any(|&(a, n)| a == atoms[2] && n == GNOME), "{names:?}");
}
