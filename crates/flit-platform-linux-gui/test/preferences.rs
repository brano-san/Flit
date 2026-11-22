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
    let worker = Watch::start(temp.path().to_owned(), Duration::from_millis(25), move |change| {
        let _ = tx.send(change);
    })
    .unwrap();
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

#[test]
fn kde_icon_theme_reads_only_the_icons_section() {
    assert_eq!(kde_icon_theme("[Other]\nTheme=Wrong\n[Icons]\nTheme = Papirus\n[Other]\nTheme=WrongAgain"), Some("Papirus".into()));
    assert_eq!(kde_icon_theme("[Icons]\nTheme=\n"), None);
    assert_eq!(kde_icon_theme("broken"), None);
}

#[test]
fn kde_icon_theme_notices_atomic_replacement_and_ignores_unrelated_changes() {
    let temp = Temp::new("icon-theme");
    let (tx, rx) = crossbeam_channel::unbounded();
    let worker = Watch::start(temp.path().to_owned(), Duration::from_millis(15), move |change| {
        let _ = tx.send(change);
    })
    .unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Buttons(_)));
    let globals = temp.path().join("kdeglobals");
    std::fs::write(&globals, "[Icons]\nTheme=First\n").unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::IconTheme("First".into()));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Globals(_)));
    std::fs::write(&globals, "[Icons]\nTheme=First\n[Other]\nTheme=Wrong\n").unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Globals(_)));
    assert!(rx.recv_timeout(Duration::from_millis(70)).is_err());
    let replacement = temp.path().join("replacement");
    std::fs::write(&replacement, "[Icons]\nTheme=Second\n").unwrap();
    std::fs::rename(&replacement, &globals).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::IconTheme("Second".into()));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Globals(_)));
    std::fs::remove_file(&globals).unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::IconTheme(KDE_ICONS.into()));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Globals(_)));
    std::fs::write(&globals, "[Icons]\nTheme=Second\n").unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::IconTheme("Second".into()));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Globals(_)));
    let at = Instant::now();
    drop(worker);
    assert!(at.elapsed() < Duration::from_secs(1));
    assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Err(crossbeam_channel::RecvTimeoutError::Disconnected));
}

#[test]
fn removing_the_kde_icon_theme_setting_goes_back_to_breeze() {
    assert_eq!(kde_icons_or_default("[General]\nColorScheme=BreezeDark\n"), "breeze");
    assert_eq!(kde_icons_or_default(""), "breeze");
    assert_eq!(kde_icons_or_default("[Icons]\nTheme=Papirus\n"), "Papirus");
    let temp = Temp::new("icon-theme-default");
    let globals = temp.path().join("kdeglobals");
    std::fs::write(&globals, "[Icons]\nTheme=Papirus\n").unwrap();
    let (tx, rx) = crossbeam_channel::unbounded();
    let worker = Watch::start(temp.path().to_owned(), Duration::from_millis(15), move |change| {
        let _ = tx.send(change);
    })
    .unwrap();
    let mut first = Vec::new();
    while let Ok(change) = rx.recv_timeout(Duration::from_millis(300)) {
        first.push(change);
    }
    assert!(first.contains(&Change::IconTheme("Papirus".into())), "{first:?}");
    std::fs::write(&globals, "[General]\nColorScheme=BreezeLight\n").unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::IconTheme("breeze".into()));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Globals(_)));
    std::fs::write(&globals, "[General]\nColorScheme=BreezeDark\n").unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Change::Globals(_)));
    assert!(rx.recv_timeout(Duration::from_millis(70)).is_err(), "an unchanged default is not announced again");
    drop(worker);
}
