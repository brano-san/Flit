use super::*;

#[test]
fn events_are_taken_per_window() {
    post(-7, HostEvent::Ready { gen: 1, ms: 2 });
    post(-8, HostEvent::Ready { gen: 3, ms: 4 });
    post(-7, HostEvent::Failed { gen: 5, message: "x".into() });
    assert_eq!(take(HWND(-7isize as _)), [HostEvent::Ready { gen: 1, ms: 2 }, HostEvent::Failed { gen: 5, message: "x".into() }]);
    assert_eq!(take(HWND(-8isize as _)), [HostEvent::Ready { gen: 3, ms: 4 }]);
    assert!(take(HWND(-7isize as _)).is_empty());
}

#[test]
fn forgetting_a_window_drops_its_undelivered_events() {
    post(-9, HostEvent::Ready { gen: 1, ms: 2 });
    forget(HWND(-9isize as _));
    assert!(take(HWND(-9isize as _)).is_empty());
}

#[test]
fn deleting_renaming_or_moving_releases_the_previewed_file_and_copying_does_not() {
    let p = |s: &str| FsPath::parse(s).unwrap();
    assert_eq!(releasing(&FileOp::Delete(vec![p(r"C:\a\f.pdf")])), [p(r"C:\a\f.pdf")]);
    assert_eq!(releasing(&FileOp::Rename { item: p(r"C:\a\f.pdf"), name: b"g.pdf".to_vec() }), [p(r"C:\a\f.pdf")]);
    assert_eq!(releasing(&FileOp::MoveTo(vec![(p(r"C:\a\f.pdf"), p(r"D:\f.pdf"))])), [p(r"C:\a\f.pdf")]);
    assert!(releasing(&FileOp::Copy { items: vec![p(r"C:\a\f.pdf")], dest: p(r"D:\") }).is_empty(), "a copy only reads, so the preview stays");
    assert!(touches(&[p(r"C:\a\f.pdf")], &p(r"c:\A\F.PDF")));
    assert!(touches(&[p(r"C:\a")], &p(r"C:\a\sub\f.pdf")), "a folder that goes away takes the preview inside it");
    assert!(touches(&[p(r"C:\")], &p(r"C:\a\f.pdf")));
    assert!(!touches(&[p(r"C:\a\f")], &p(r"C:\a\f.pdf")) && !touches(&[p(r"C:\a\b")], &p(r"C:\a\bc\f.pdf")), "a name that only starts the same is left alone");
}

#[test]
fn video_uses_the_available_device_and_retries_unsupported_devices_in_the_window() {
    let unsupported = windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL);
    let render = |device: Option<&u8>| Ok(device.copied());
    assert_eq!(video_or_legacy(Ok(7), render).unwrap(), Some(7));
    assert_eq!(video_or_legacy(Err(unsupported.clone()), render).unwrap(), None);
    let mut attempts = Vec::new();
    video_or_legacy(Ok(7), |device| {
        attempts.push(device.copied());
        if device.is_some() {
            Err(unsupported.clone())
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(attempts, [Some(7), None]);
}
