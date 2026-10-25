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
