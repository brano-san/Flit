use super::*;

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn programs_with_the_item_or_something_inside_it_open_are_named_once() {
    let procs = vec![
        ("vim".to_owned(), vec![p("/dev/pts/1"), p("/media/usb/notes.txt")]),
        ("bash".to_owned(), vec![p("/media/usb/photos")]),
        ("vim".to_owned(), vec![p("/media/usb/todo.txt")]),
        ("cat".to_owned(), vec![p("/media/usbstick/a")]),
        ("ls".to_owned(), vec![p("/home/me")]),
    ];
    assert_eq!(holders_in(&p("/media/usb"), procs.clone()), ["vim", "bash"]);
    assert_eq!(holders_in(&p("/media/usb/notes.txt"), procs.clone()), ["vim"]);
    assert!(holders_in(&p("/media/us"), procs).is_empty(), "a name that only starts the same is not held");
}

#[test]
fn this_process_is_found_holding_a_file_it_has_open() {
    let t = crate::temp::Temp::new("busy");
    let file = t.file("held", 1);
    let other = t.file("free", 1);
    let held = std::fs::File::open(&file).unwrap();
    let me = std::fs::read_to_string("/proc/self/comm").unwrap().trim_end().to_owned();
    assert!(holders(&file).contains(&me), "{:?}", holders(&file));
    assert!(!holders(&other).contains(&me));
    drop(held);
}
