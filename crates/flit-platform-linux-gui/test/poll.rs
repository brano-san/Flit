use super::*;
use crate::temp::Temp;
use crossbeam_channel::Receiver;
use std::sync::Arc;

const FAST: Duration = Duration::from_millis(40);

fn poll() -> (Poll, Receiver<ProviderEvent>) {
    let (tx, rx) = unbounded();
    (Poll::new(tx, Arc::new(|| {}), FAST), rx)
}

fn rescans(rx: &Receiver<ProviderEvent>, wait: Duration) -> Vec<(TabId, u64)> {
    let until = Instant::now() + wait;
    let mut out = Vec::new();
    while let Ok(e) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
        if let ProviderEvent::Rescan { tab, gen } = e {
            out.push((tab, gen));
        }
    }
    out
}

#[test]
fn a_polled_folder_reloads_once_after_something_changes_in_it() {
    let t = Temp::new("poll-change");
    let (mut p, rx) = poll();
    p.watch(4, 7, t.path());
    assert!(rescans(&rx, FAST * 5).is_empty(), "an unchanged folder is not reloaded");
    std::thread::sleep(Duration::from_millis(20));
    t.file("from-another-machine.txt", 3);
    assert_eq!(rescans(&rx, FAST * 6), [(4, 7)]);
    std::fs::remove_dir_all(t.path()).unwrap();
    assert_eq!(rescans(&rx, FAST * 6), [(4, 7)], "a folder that went away reloads once to show the error");
}

#[test]
fn a_forgotten_or_replaced_watch_no_longer_reloads_the_old_view() {
    let (a, b) = (Temp::new("poll-a"), Temp::new("poll-b"));
    let (mut p, rx) = poll();
    assert!(!p.running());
    p.forget(1);
    assert!(!p.running(), "forgetting before any watch starts no thread");
    p.watch(1, 1, a.path());
    p.watch(2, 1, b.path());
    p.forget(1);
    p.watch(2, 2, b.path());
    std::thread::sleep(FAST * 2);
    std::thread::sleep(Duration::from_millis(20));
    a.file("x", 1);
    b.file("y", 1);
    assert_eq!(rescans(&rx, FAST * 6), [(2, 2)]);
}
