use super::*;

#[test]
fn nodes_link_to_their_parent_and_siblings() {
    let mut t = Tree::new("Flit", Rect::new(0.0, 0.0, 100.0, 100.0));
    let bar = t.add(0, 2, Role::ToolBar, "Bar", Rect::new(0.0, 0.0, 100.0, 20.0));
    let a = t.add(bar, 10, Role::Button, "A", Rect::new(0.0, 0.0, 50.0, 20.0));
    let b = t.add(bar, 11, Role::Button, "B", Rect::new(50.0, 0.0, 50.0, 20.0));
    assert_eq!(t.nodes[0].children, vec![bar]);
    assert_eq!(t.nodes[a].parent, bar);
    assert_eq!(t.sibling(a, 1), Some(b));
    assert_eq!(t.sibling(b, 1), None);
    assert_eq!(t.sibling(a, -1), None);
    assert_eq!(t.sibling(0, 1), None);
    assert_eq!(t.find(11), Some(b));
    assert_eq!(t.at(60.0, 10.0), b);
    assert_eq!(t.at(60.0, 50.0), 0);
}