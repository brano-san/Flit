use super::*;

fn s(f: impl FnOnce(&mut String)) -> String {
    let mut o = String::new();
    f(&mut o);
    o
}

#[test]
fn sizes() {
    assert_eq!(s(|o| size(o, 0)), "0 B");
    assert_eq!(s(|o| size(o, 1023)), "1023 B");
    assert_eq!(s(|o| size(o, 1536)), "1.5 KB");
    assert_eq!(s(|o| size(o, 150 * 1024 * 1024)), "150 MB");
}

#[test]
fn dates() {
    let unix = 1_792_891_860i64;
    let ft = (unix + 11_644_473_600) * 10_000_000;
    assert_eq!(s(|o| datetime(o, ft, 0)), "2026-10-25 01:31");
    assert_eq!(s(|o| datetime(o, ft, 180)), "2026-10-25 04:31");
    assert_eq!(s(|o| datetime(o, ft, -120)), "2026-10-24 23:31");
    assert_eq!(s(|o| datetime(o, 0, 0)), "");
}
