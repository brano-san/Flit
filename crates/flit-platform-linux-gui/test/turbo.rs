use super::*;

const EIGHTHS: [(c_int, c_int); 8] = [(1, 8), (1, 4), (3, 8), (1, 2), (5, 8), (3, 4), (7, 8), (1, 1)];

#[test]
fn the_smallest_scale_that_still_covers_the_thumbnail_is_chosen() {
    assert_eq!(scale_for(4000, 3000, 256, &EIGHTHS), (500, 375));
    assert_eq!(scale_for(4000, 3000, 600, &EIGHTHS), (1000, 750));
    assert_eq!(scale_for(3000, 4000, 1024, &EIGHTHS), (1125, 1500));
    assert_eq!(scale_for(200, 100, 256, &EIGHTHS), (200, 100));
    assert_eq!(scale_for(1001, 7, 96, &EIGHTHS), (126, 1));
}

#[test]
fn libturbojpeg_decodes_like_jpeg_decoder_when_it_is_installed() {
    let t = crate::temp::Temp::new("turbo");
    let src = t.path().join("g.jpg");
    let made = std::process::Command::new("convert").args(["-size", "800x600", "gradient:red-blue", src.to_str().unwrap()]).status().is_ok_and(|s| s.success());
    if !made {
        return;
    }
    let data = std::fs::read(&src).unwrap();
    let Some(b) = decode(&data, 96) else {
        assert!(api().is_none(), "libturbojpeg is loaded but did not decode");
        return;
    };
    assert_eq!((b.width, b.height), (100, 75));
    assert_eq!(b.rgba.len(), 100 * 75 * 4);
    assert!(b.rgba[0] > 200 && b.rgba[2] < 60 && b.rgba[3] == 255, "the top is red: {:?}", &b.rgba[..4]);
    let last = b.rgba.len() - 4;
    assert!(b.rgba[last] < 60 && b.rgba[last + 2] > 200, "the bottom is blue: {:?}", &b.rgba[last..]);
    assert!(decode(b"not a jpeg", 96).is_none());
}
