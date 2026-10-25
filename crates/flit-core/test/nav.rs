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

#[test]
fn jumps_several_steps_and_lists_both_stacks() {
    let p = |s| Location::parse(s).unwrap();
    let mut h = History::new(p(r"C:\"));
    for s in [r"C:\a", r"C:\b", r"C:\c"] {
        h.go(p(s));
    }
    assert!(!h.jump(-4), "more steps than the back stack holds");
    assert!(!h.jump(0));
    assert!(h.jump(-2));
    assert_eq!(h.current(), &p(r"C:\a"));
    assert_eq!(h.back_list(), &[p(r"C:\")]);
    assert_eq!(h.forward_list(), &[p(r"C:\c"), p(r"C:\b")]);
    assert!(h.jump(2));
    assert_eq!(h.current(), &p(r"C:\c"));
    h.replace(p(r"D:\"));
    assert_eq!(h.current(), &p(r"D:\"));
    assert_eq!(h.back_list().len(), 3, "replacing keeps the history around it");
}
