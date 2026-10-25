use super::*;

fn px(w: u32, h: u32) -> Vec<u8> {
    vec![255; (w * h * 4) as usize]
}

fn drawn(a: &mut ThumbAtlas, key: u64, r: Rect) -> Option<Rect> {
    let mut dl = DrawList::default();
    a.draw(&mut dl, key, r, 255).then(|| {
        let i = dl.instances[0];
        assert_eq!(i.kind, crate::render::kind::THUMB);
        Rect::new(i.rect[0], i.rect[1], i.rect[2], i.rect[3])
    })
}

#[test]
fn thumbnails_fit_their_box_and_never_share_texture_space() {
    let mut a = ThumbAtlas::default();
    assert!(a.insert(1, 200, 100, &px(200, 100)));
    assert!(a.insert(2, 64, 64, &px(64, 64)));
    assert!(a.insert(3, 500, 300, &px(500, 300)));
    assert!(!a.insert(4, 600, 10, &px(600, 10)), "larger than a page");
    assert!(!a.insert(5, 10, 10, &px(3, 3)), "wrong pixel count");
    let slots: Vec<Slot> = a.uploads.rects.iter().map(|r| r.0).collect();
    assert_eq!(slots.len(), 3);
    for (i, p) in slots.iter().enumerate() {
        assert!(p.x + p.w <= THUMB_ATLAS && p.y + p.h <= THUMB_ATLAS);
        for q in &slots[i + 1..] {
            assert!(p.x + p.w <= q.x || q.x + q.w <= p.x || p.y + p.h <= q.y || q.y + q.h <= p.y, "{p:?} overlaps {q:?}");
        }
    }
    assert_eq!(drawn(&mut a, 1, Rect::new(10.0, 10.0, 100.0, 100.0)), Some(Rect::new(10.0, 35.0, 100.0, 50.0)));
    assert_eq!(drawn(&mut a, 2, Rect::new(0.0, 0.0, 128.0, 128.0)), Some(Rect::new(32.0, 32.0, 64.0, 64.0)), "small pictures are not blown up");
    assert_eq!(drawn(&mut a, 9, Rect::new(0.0, 0.0, 10.0, 10.0)), None);
    assert_eq!((class_px(10), class_px(65), class_px(256), class_px(4000)), (64, 128, 256, 512));
}

#[test]
fn the_least_recently_drawn_thumbnails_make_room_but_this_frames_never_do() {
    let mut a = ThumbAtlas::default();
    let pages = (PAGES_PER_ROW * PAGES_PER_ROW) as u64;
    for k in 1..=pages {
        assert!(a.insert(k, 512, 512, &px(512, 512)));
    }
    assert!(!a.insert(pages + 1, 512, 512, &px(512, 512)), "everything was added this frame");
    a.begin_frame();
    let r = Rect::new(0.0, 0.0, 512.0, 512.0);
    for k in 2..=pages {
        drawn(&mut a, k, r).unwrap();
    }
    assert!(a.insert(pages + 1, 512, 512, &px(512, 512)));
    assert!(!a.contains(1) && a.contains(pages + 1) && a.len() == pages as usize);
    a.begin_frame();
    for k in 2..=pages {
        drawn(&mut a, k, r).unwrap();
    }
    assert!(a.insert(pages + 2, 40, 40, &px(40, 40)), "a stale page changes size class");
    assert!(!a.contains(pages + 1) && a.contains(pages + 2));
    assert!(a.insert(pages + 3, 40, 40, &px(40, 40)), "the converted page holds many small thumbnails");
    a.clear();
    assert!(a.is_empty() && a.uploads.rects.is_empty() && a.insert(7, 512, 512, &px(512, 512)));
}
