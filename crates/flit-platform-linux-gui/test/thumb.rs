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

fn with_exif_thumbnail(main: &[u8], thumb: &[u8], le: bool) -> Vec<u8> {
    let u16b = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let u32b = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let mut tiff = Vec::new();
    tiff.extend_from_slice(if le { b"II" } else { b"MM" });
    tiff.extend_from_slice(&u16b(42));
    tiff.extend_from_slice(&u32b(8));
    tiff.extend_from_slice(&u16b(0));
    tiff.extend_from_slice(&u32b(14));
    tiff.extend_from_slice(&u16b(2));
    for (tag, value) in [(0x0201u16, 14 + 2 + 24 + 4), (0x0202, thumb.len() as u32)] {
        tiff.extend_from_slice(&u16b(tag));
        tiff.extend_from_slice(&u16b(4));
        tiff.extend_from_slice(&u32b(1));
        tiff.extend_from_slice(&u32b(value));
    }
    tiff.extend_from_slice(&u32b(0));
    tiff.extend_from_slice(thumb);
    let mut out = main[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(&tiff);
    out.extend_from_slice(&main[2..]);
    out
}

#[test]
fn an_embedded_exif_thumbnail_is_used_when_it_is_large_enough() {
    let t = Temp::new("thumb-exif");
    let (big, small) = (t.path().join("big.jpg"), t.path().join("small.jpg"));
    if !convert(&["-size", "800x600", "xc:red", big.to_str().unwrap()]) || !convert(&["-size", "160x120", "xc:blue", small.to_str().unwrap()]) {
        return;
    }
    for le in [true, false] {
        let joined = with_exif_thumbnail(&std::fs::read(&big).unwrap(), &std::fs::read(&small).unwrap(), le);
        assert_eq!(exif_thumbnail(&joined).map(|t| t.len()), Some(std::fs::metadata(&small).unwrap().len() as usize));
        let src = t.path().join("joined.jpg");
        std::fs::write(&src, &joined).unwrap();
        let blue = thumbnail(&src, 96, None).unwrap();
        assert_eq!((blue.width, blue.height), (96, 72));
        assert!(blue.rgba[2] > 200 && blue.rgba[0] < 60, "the embedded blue thumbnail: {:?}", &blue.rgba[..4]);
        let red = thumbnail(&src, 256, None).unwrap();
        assert_eq!((red.width, red.height), (256, 192));
        assert!(red.rgba[0] > 200, "the full red image when the embedded one is too small");
    }
    assert!(exif_thumbnail(&std::fs::read(&big).unwrap()).is_none());
}

fn with_orientation(main: &[u8], o: u16) -> Vec<u8> {
    let mut tiff = b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01".to_vec();
    tiff.extend_from_slice(&o.to_be_bytes());
    tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut out = main[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(&tiff);
    out.extend_from_slice(&main[2..]);
    out
}

#[test]
fn exif_orientation_turns_the_picture() {
    let px = |b: &Bitmap, x: u32, y: u32| b.rgba[((y * b.width + x) * 4) as usize];
    let src = Bitmap { width: 3, height: 2, rgba: (0..6u8).flat_map(|v| [v, 0, 0, 255]).collect() };
    let cw = orient(src.clone(), 6);
    assert_eq!((cw.width, cw.height), (2, 3));
    assert_eq!((px(&cw, 0, 0), px(&cw, 1, 0), px(&cw, 0, 2)), (3, 0, 5));
    let ccw = orient(src.clone(), 8);
    assert_eq!((px(&ccw, 0, 0), px(&ccw, 1, 0)), (2, 5));
    assert_eq!(px(&orient(src.clone(), 3), 0, 0), 5);
    assert_eq!(px(&orient(src.clone(), 2), 0, 0), 2);
    assert_eq!(px(&orient(src.clone(), 4), 0, 0), 3);
    assert_eq!(orient(src.clone(), 1).rgba, src.rgba);
    let t = Temp::new("orient");
    let plain = t.path().join("p.jpg");
    if !convert(&["-size", "64x32", "xc:red", plain.to_str().unwrap()]) {
        return;
    }
    let turned = with_orientation(&std::fs::read(&plain).unwrap(), 6);
    assert_eq!(orientation(&turned), 6);
    assert_eq!(orientation(&std::fs::read(&plain).unwrap()), 1);
    let file = t.path().join("turned.jpg");
    std::fs::write(&file, &turned).unwrap();
    let b = image(Path::new(&file), 1000).unwrap();
    assert_eq!((b.width, b.height), (32, 64));
    let thumb = thumbnail(&file, 16, None).unwrap();
    assert_eq!((thumb.width, thumb.height), (8, 16));
}

#[test]
#[ignore]
fn bench_a_twelve_megapixel_jpeg_thumbnail() {
    let t = Temp::new("thumb-bench");
    let sources: [(&str, &[&str]); 2] = [("noise", &["plasma:"]), ("smooth", &["plasma:", "-blur", "0x3"])];
    for (label, make) in sources {
        let src = t.path().join(format!("{label}.jpg"));
        let mut args = vec!["-size", "4000x3000"];
        args.extend_from_slice(make);
        args.extend_from_slice(&["-quality", "90", src.to_str().unwrap()]);
        assert!(convert(&args));
        let mut took: Vec<_> = (0..7)
            .map(|_| {
                let start = Instant::now();
                assert!(thumbnail(&src, 256, None).is_some());
                start.elapsed()
            })
            .collect();
        took.sort();
        eprintln!("measure: 12 MP {label} JPEG of {} bytes to a 256 px thumbnail median {:?}", std::fs::metadata(&src).unwrap().len(), took[3]);
    }
}
