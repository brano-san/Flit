use super::*;
use crate::temp::Temp;
use std::time::Instant;

fn write_png(path: &Path, w: u32, h: u32, text: Option<(&str, String)>) {
    let mut e = png::Encoder::new(File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    if let Some((k, v)) = text {
        e.add_text_chunk(k.into(), v).unwrap();
    }
    let data: Vec<u8> = (0..w * h).flat_map(|i| [(i % 256) as u8, 7, 200]).collect();
    e.write_header().unwrap().write_image_data(&data).unwrap();
}

fn convert(args: &[&str]) -> bool {
    std::process::Command::new("convert").args(args).status().is_ok_and(|s| s.success())
}

#[test]
fn freedesktop_names_follow_the_md5_of_the_file_uri() {
    assert_eq!(freedesktop_name(Path::new("/home/jens/photos/me.png")), "c6ee772d9e49320e97ec29a7eb5b1697.png");
}

#[test]
fn pngs_shrink_to_fit_and_fresh_freedesktop_thumbnails_win() {
    let t = Temp::new("thumb");
    let src = t.path().join("Big.PNG");
    write_png(&src, 400, 200, None);
    let b = thumbnail(&src, 96, None).unwrap();
    assert_eq!((b.width, b.height, b.rgba.len()), (96, 48, 96 * 48 * 4));
    assert_eq!(b.rgba[3], 255);
    let cache = t.path().join("thumbnails");
    std::fs::create_dir_all(cache.join("normal")).unwrap();
    let mtime = std::fs::metadata(&src).unwrap().mtime().to_string();
    write_png(&cache.join("normal").join(freedesktop_name(&src)), 10, 5, Some(("Thumb::MTime", mtime)));
    assert_eq!(thumbnail(&src, 96, Some(cache.clone())).map(|b| (b.width, b.height)), Some((10, 5)));
    write_png(&cache.join("normal").join(freedesktop_name(&src)), 10, 5, Some(("Thumb::MTime", "1".into())));
    assert_eq!(thumbnail(&src, 96, Some(cache)).map(|b| (b.width, b.height)), Some((96, 48)), "a stale cached thumbnail is ignored");
    assert!(thumbnail(&t.path().join("x.txt"), 96, None).is_none());
    std::fs::write(t.path().join("bad.jpg"), b"not a jpeg").unwrap();
    assert!(thumbnail(&t.path().join("bad.jpg"), 96, None).is_none());
}

#[test]
fn jpegs_decode_at_a_reduced_scale() {
    let t = Temp::new("thumb-jpeg");
    let src = t.path().join("p.jpg");
    if !convert(&["-size", "640x480", "gradient:red-blue", src.to_str().unwrap()]) {
        return;
    }
    let b = thumbnail(&src, 96, None).unwrap();
    assert_eq!((b.width, b.height), (96, 72));
    assert!(b.rgba[0] > 200 && b.rgba[2] < 60, "the top is red: {:?}", &b.rgba[..4]);
}

#[test]
#[ignore]
fn bench_a_twelve_megapixel_jpeg_thumbnail() {
    let t = Temp::new("thumb-bench");
    let src = t.path().join("p.jpg");
    assert!(convert(&["-size", "4000x3000", "plasma:", "-quality", "90", src.to_str().unwrap()]));
    let mut took: Vec<_> = (0..7)
        .map(|_| {
            let start = Instant::now();
            assert!(thumbnail(&src, 256, None).is_some());
            start.elapsed()
        })
        .collect();
    took.sort();
    eprintln!("measure: 12 MP JPEG to a 256 px thumbnail median {:?}", took[3]);
}
