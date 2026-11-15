use super::*;
use flit_core::ui::IconKind;

#[test]
fn queued_content_requests_follow_exact_visible_keys_even_without_a_generation() {
    let visible = Visible::default();
    let req = IconRequest { key: 7, kind: IconKind::Content, path: flit_core::FsPath::parse("/tmp/image").unwrap(), size: 16, gen: 0, row: 0 };
    assert!(!visible.wants(&req));
    *visible.content.lock().unwrap() = vec![7];
    assert!(visible.wants(&req));
    assert!(!Visible::default().wants(&req), "another window has its own visibility");
    *visible.content.lock().unwrap() = vec![9];
    assert!(!visible.wants(&req), "scrolling away cancels a queued gen=0 search or column request");
    assert!(visible.wants(&IconRequest { kind: IconKind::Type, ..req }));
}
