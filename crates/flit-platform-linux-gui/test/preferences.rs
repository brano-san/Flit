use super::*;
use crate::temp::Temp;
use flit_core::input::WindowButton::{Close, Maximize, Minimize};
use std::time::Instant;

#[test]
fn gnome_buttons_follow_each_side_and_do_not_invent_missing_controls() {
    assert_eq!(gnome_buttons("appmenu:minimize,maximize,close"), WindowButtons::default());
    assert_eq!(gnome_buttons("close,minimize,maximize:"), WindowButtons { left: vec![Close, Minimize, Maximize], right: vec![] });
    for text in ["", ":", "appmenu,icon:menu,spacer"] {
        assert_eq!(gnome_buttons(text), WindowButtons { left: vec![], right: vec![] });
    }
    assert_eq!(gnome_buttons("close,close:minimize,close,minimize"), WindowButtons { left: vec![Close], right: vec![Minimize] });
    assert_eq!(gnome_buttons("close:maximize"), WindowButtons { left: vec![Close], right: vec![Maximize] });
}

#[test]
fn kde_buttons_respect_the_decoration_section_and_explicit_empty_values() {
    assert_eq!(kde_buttons(""), WindowButtons::default());
    assert_eq!(kde_buttons("[Other]\nButtonsOnLeft=X\nButtonsOnRight=\n"), WindowButtons::default());
    assert_eq!(kde_buttons("[org.kde.kdecoration2]\nButtonsOnLeft=XIA\nButtonsOnRight=\n"), WindowButtons { left: vec![Close, Minimize, Maximize], right: vec![] });
    assert_eq!(kde_buttons("[org.kde.kdecoration2]\nButtonsOnLeft=XMXS\nButtonsOnRight=HIIAX\n"), WindowButtons { left: vec![Close], right: vec![Minimize, Maximize] });
    assert_eq!(kde_buttons("[org.kde.kdecoration2]\nButtonsOnLeft=\nButtonsOnRight=X\n[Other]\nButtonsOnRight=IA\n"), WindowButtons { left: vec![], right: vec![Close] });
    assert_eq!(Desktop::from_name("ubuntu:GNOME"), Desktop::Gnome);
    assert_eq!(Desktop::from_name("KDE"), Desktop::Kde);
    assert_eq!(Desktop::from_name("X-Cinnamon"), Desktop::Other);
}

#[test]
fn a_kde_worker_notices_atomic_replacement_deletion_and_recreation_then_stops() {
    let temp = Temp::new("window-buttons");
    let (tx, rx) = crossbeam_channel::unbounded();
    let worker = Watch::start(temp.path().to_owned(), Duration::from_millis(25), move |change| { let _ = tx.send(change); }).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Buttons(WindowButtons::default()));
    std::fs::write(temp.path().join("replacement"), "[org.kde.kdecoration2]\nButtonsOnLeft=X\nButtonsOnRight=\n").unwrap();
    std::fs::rename(temp.path().join("replacement"), temp.path().join("kwinrc")).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Buttons(WindowButtons { left: vec![Close], right: vec![] }));
    std::fs::write(temp.path().join("unrelated"), "x").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    std::fs::remove_file(temp.path().join("kwinrc")).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Buttons(WindowButtons::default()));
    std::fs::write(temp.path().join("kwinrc"), "[org.kde.kdecoration2]\nButtonsOnRight=X\n").unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Buttons(WindowButtons { left: vec![], right: vec![Close] }));
    std::fs::write(temp.path().join("kdeglobals"), "[Colors:Window]\nBackgroundNormal=1,2,3\n").unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Globals(s) if s.contains("1,2,3")));
    let start = Instant::now();
    drop(worker);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Err(crossbeam_channel::RecvTimeoutError::Disconnected));
}
