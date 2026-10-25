use super::*;

#[test]
fn packs_without_overlap_until_full() {
    let mut a = Atlas::new(64);
    let mut slots = Vec::new();
    while let Some(s) = a.alloc(7, 9) {
        slots.push(s);
    }
    assert_eq!(slots.len(), 8 * 6);
    for (i, p) in slots.iter().enumerate() {
        assert!(p.x + p.w <= 64 && p.y + p.h <= 64);
        for q in &slots[i + 1..] {
            let disjoint = p.x + p.w <= q.x || q.x + q.w <= p.x || p.y + p.h <= q.y || q.y + q.h <= p.y;
            assert!(disjoint, "{p:?} overlaps {q:?}");
        }
    }
    a.clear();
    assert!(a.alloc(63, 63).is_some());
}
