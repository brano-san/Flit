use super::*;

fn view(v: u8) -> Restore {
    Restore { select: vec![vec![v]], focus: Some(vec![v]), scroll: f32::from(v), scroll_target: f32::from(v) }
}

#[test]
fn back_forward() {
    let p = |s| Location::parse(s).unwrap();
    let mut h = History::new(p(r"C:\"));
    assert!(h.go(p(r"C:\a"), None));
    assert!(h.go(p("wsl://Ubuntu/home"), None));
    assert!(!h.go(p(r"\\wsl$\Ubuntu\home"), None), "same location in another spelling");
    assert_eq!(h.back(None).unwrap(), &p(r"C:\a"));
    assert_eq!(h.back(None).unwrap(), &p(r"C:\"));
    assert!(h.back(None).is_none());
    assert_eq!(h.forward(None).unwrap(), &p(r"C:\a"));
    h.go(p(r"D:\"), None);
    assert!(!h.can_forward());
    assert_eq!(h.back(None).unwrap(), &p(r"C:\a"));
}

#[test]
fn jumps_several_steps_and_lists_both_stacks() {
    let p = |s| Location::parse(s).unwrap();
    let mut h = History::new(p(r"C:\"));
    for s in [r"C:\a", r"C:\b", r"C:\c"] {
        h.go(p(s), None);
    }
    assert!(!h.jump(-4, None), "more steps than the back stack holds");
    assert!(!h.jump(0, None));
    assert!(h.jump(-2, None));
    assert_eq!(h.current(), &p(r"C:\a"));
    assert_eq!(h.back_list(), &[p(r"C:\")]);
    assert_eq!(h.forward_list(), &[p(r"C:\c"), p(r"C:\b")]);
    assert!(h.jump(2, None));
    assert_eq!(h.current(), &p(r"C:\c"));
    h.replace(p(r"D:\"));
    assert_eq!(h.current(), &p(r"D:\"));
    assert_eq!(h.back_list().len(), 3, "replacing keeps the history around it");
}

#[test]
fn leaving_a_folder_keeps_its_view_for_the_return_trip() {
    let p = |s| Location::parse(s).unwrap();
    let mut h = History::new(p(r"C:\"));
    h.go(p(r"C:\a"), None);
    h.go(p(r"C:\b"), Some(view(1)));
    assert!(h.take_here().is_none());
    assert_eq!(h.back(Some(view(2))).unwrap(), &p(r"C:\a"));
    assert_eq!(h.take_here(), Some(view(1)), "the view saved when the folder was left comes back with it");
    assert!(h.take_here().is_none());
    assert_eq!(h.forward(Some(view(1))).unwrap(), &p(r"C:\b"));
    assert_eq!(h.take_here(), Some(view(2)));
    h.go(p(r"C:\c"), Some(view(2)));
    assert!(h.take_here().is_none(), "a fresh navigation has no saved view");
}

#[test]
fn a_long_jump_does_not_reuse_the_departure_view_for_other_stops() {
    let p = |s| Location::parse(s).unwrap();
    let mut h = History::new(p(r"C:\"));
    for s in [r"C:\a", r"C:\b", r"C:\c"] {
        h.go(p(s), None);
    }
    assert!(h.jump(-2, Some(view(3))));
    assert!(h.take_here().is_none(), "the far stop has no saved view of its own");
    assert_eq!(h.current(), &p(r"C:\a"));
    assert!(h.jump(2, None));
    assert_eq!(h.current(), &p(r"C:\c"));
    assert_eq!(h.take_here(), Some(view(3)), "the stop keeps the view saved when it was left");
}

#[test]
fn retract_and_replace_drop_saved_views() {
    let p = |s| Location::parse(s).unwrap();
    let mut h = History::new(p(r"C:\"));
    h.go(p(r"C:\a"), Some(view(1)));
    h.go(p(r"C:\b"), None);
    assert!(h.retract());
    assert!(h.take_here().is_none());
    assert_eq!(h.current(), &p(r"C:\a"));
    h.go(p(r"C:\b"), Some(view(1)));
    h.back(None);
    assert_eq!(h.take_here(), Some(view(1)));
    h.replace(p(r"C:\z"));
    assert!(h.take_here().is_none());
}
