use super::*;
use flit_core::ui::IconRequest;

fn solid(size: u32, px: [u8; 4]) -> Bitmap {
    Bitmap { width: size, height: size, rgba: px.repeat((size * size) as usize) }
}

#[test]
fn fit_never_exceeds_the_requested_size() {
    let b = fit(solid(256, [10, 20, 30, 40]), 56);
    assert_eq!((b.width, b.height, b.rgba.len()), (56, 56, 56 * 56 * 4));
    assert!(b.rgba.chunks_exact(4).all(|p| p == [10, 20, 30, 40]));
    let small = fit(solid(48, [1, 2, 3, 4]), 56);
    assert_eq!((small.width, small.height), (48, 48));
}

#[test]
fn fit_box_filters_premultiplied_pixels() {
    let mut b = solid(4, [0, 0, 0, 0]);
    for y in 0..2 {
        for x in 0..2 {
            let p = ((y * 4 + x) * 4) as usize;
            b.rgba[p..p + 4].copy_from_slice(&[200, 100, 0, 200]);
        }
    }
    let half = fit(b, 2);
    assert_eq!(&half.rgba[..4], &[200, 100, 0, 200]);
    assert_eq!(&half.rgba[4..8], &[0, 0, 0, 0]);
    let quarter = fit(half, 1);
    assert_eq!(quarter.rgba, vec![50, 25, 0, 50]);
}

#[test]
fn shell_icons_match_requested_size() {
    let mut shell = ShellCom::new().unwrap();
    for size in [16, 56, 100, 300] {
        let req = IconRequest { key: 0, kind: IconKind::Type, path: FsPath::parse(r"C:\x.txt").unwrap(), size, gen: 0, row: 0 };
        let b = shell.icon(&req).expect("type icon");
        assert!(b.width <= size && b.height <= size, "{size}: {}x{}", b.width, b.height);
        assert_eq!(b.rgba.len(), (b.width * b.height * 4) as usize);
    }
}
