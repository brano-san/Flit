use super::*;

#[test]
fn back_forward() {
    let p = |s| Location::parse(s).unwrap();
    let mut h = History::new(p(r"C:\"));
    assert!(h.go(p(r"C:\a")));
    assert!(h.go(p("wsl://Ubuntu/home")));
    assert!(!h.go(p(r"\\wsl$\Ubuntu\home")), "same location in another spelling");
    assert_eq!(h.back().unwrap(), &p(r"C:\a"));
    assert_eq!(h.back().unwrap(), &p(r"C:\"));
    assert!(h.back().is_none());
    assert_eq!(h.forward().unwrap(), &p(r"C:\a"));
    h.go(p(r"D:\"));
    assert!(!h.can_forward());
    assert_eq!(h.back().unwrap(), &p(r"C:\a"));
}
