use super::*;
use crate::temp::Temp;
use std::os::unix::fs::PermissionsExt;

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn run(t: &Thumbnailer, path: &Path, size: u32) -> Option<Bitmap> {
    crate::thumb::png_bytes(&run_png(t, path, size)?).map(|b| crate::thumb::fit(b, size))
}

fn png(path: &Path, w: u32, h: u32) {
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&vec![90; (w * h * 3) as usize]).unwrap();
}

#[test]
fn entries_need_their_program_and_list_their_types() {
    let text = "[Thumbnailer Entry]\nTryExec=/bin/sh\nExec=/bin/sh -s %s %u %o\nMimeType=image/webp;image/heif;\n";
    assert_eq!(parse(text), Some(Thumbnailer { exec: "/bin/sh -s %s %u %o".into(), mimes: vec!["image/webp".into(), "image/heif".into()] }));
    assert_eq!(parse(&text.replace("TryExec=/bin/sh", "TryExec=/no/such/thumbnailer")), None, "a missing program is skipped");
    assert_eq!(parse("[Other]\nExec=/bin/sh\n"), None);
    let all = [parse(text).unwrap()];
    assert!(pick(&all, "IMAGE/HEIF").is_some() && pick(&all, "image/png").is_none());
}

#[test]
fn codes_in_the_command_become_the_file_its_uri_the_output_and_the_size() {
    let args = command("thumb -s %s %u %i --out=%o 100%%", Path::new("/p/a b.heic"), Path::new("/tmp/o.png"), 256);
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args, ["thumb", "-s", "256", "file:///p/a%20b.heic", "/p/a b.heic", "--out=/tmp/o.png", "100%"]);
}

#[test]
fn the_first_data_directory_overrides_a_thumbnailer_of_the_same_name() {
    let t = Temp::new("thumbnailers");
    let (user, system) = (t.path().join("user"), t.path().join("system"));
    for (dir, exec) in [(&user, "/bin/sh user"), (&system, "/bin/sh system")] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("x.thumbnailer"), format!("[Thumbnailer Entry]\nExec={exec}\nMimeType=image/webp;\n")).unwrap();
    }
    std::fs::write(system.join("y.thumbnailer"), "[Thumbnailer Entry]\nExec=/bin/sh y\nMimeType=image/avif;\n").unwrap();
    std::fs::write(system.join("notes.txt"), "[Thumbnailer Entry]\nExec=/bin/sh z\n").unwrap();
    let all = load(&[user, system, t.path().join("missing")]);
    assert_eq!(all.iter().map(|t| t.exec.as_str()).collect::<Vec<_>>(), ["/bin/sh user", "/bin/sh y"]);
}

#[test]
fn a_thumbnailer_writes_a_png_that_is_read_back_fitted_and_removed() {
    let t = Temp::new("thumbnailer-run");
    let src = t.path().join("photo.heic");
    png(&src, 300, 150);
    let copy = script(t.path(), "copy", "cp \"$1\" \"$2\"");
    let ok = Thumbnailer { exec: format!("{} %i %o", copy.display()), mimes: vec!["image/heif".into()] };
    for attempt in 0..3 {
        match run(&ok, &src, 100) {
            Some(b) => {
                assert_eq!((b.width, b.height), (100, 50));
                break;
            }
            None if attempt < 2 => {}
            None => panic!("the thumbnailer produced nothing in every attempt"),
        }
    }
    let leftovers = std::fs::read_dir(std::env::temp_dir()).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with(&format!("flit-thumb-{}-", std::process::id()))).count();
    assert_eq!(leftovers, 0, "the output file is removed");
    let fails = script(t.path(), "fails", "cp \"$1\" \"$2\"; exit 1");
    assert!(run(&Thumbnailer { exec: format!("{} %i %o", fails.display()), ..ok }, &src, 100).is_none(), "a failed run shows no picture");
}

#[test]
fn the_installed_gdk_pixbuf_thumbnailer_draws_a_bmp() {
    let Some(program) = crate::shell::on_path(std::ffi::OsStr::new("gdk-pixbuf-thumbnailer")) else { return };
    let t = Temp::new("thumbnailer-bmp");
    let src = t.path().join("p.bmp");
    let (w, h) = (64u32, 32u32);
    let size = 54 + w * h * 4;
    let mut bmp = Vec::new();
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&size.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(w as i32).to_le_bytes());
    bmp.extend_from_slice(&(h as i32).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    bmp.extend((0..w * h).flat_map(|_| [200u8, 100, 50, 255]));
    std::fs::write(&src, bmp).unwrap();
    let real = Thumbnailer { exec: format!("{} -s %s %u %o", program.display()), mimes: vec!["image/bmp".into()] };
    let b = run(&real, &src, 32).expect("gdk-pixbuf-thumbnailer made a thumbnail");
    assert_eq!(b.width.max(b.height), 32);
}

fn png_data(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&vec![90; (w * h * 3) as usize]).unwrap();
    out
}

#[test]
fn a_generated_thumbnail_goes_to_the_shared_cache_where_flit_and_other_apps_find_it() {
    let t = Temp::new("thumb-cache");
    let cache = t.path().join("thumbnails");
    let book = t.file("book.epub", 9);
    let asked = std::cell::Cell::new(0);
    let made = produce(&book, 96, Some(&cache), |px| {
        asked.set(px);
        Some(png_data(px, px / 2))
    });
    assert_eq!(asked.get(), 128, "a list thumbnail is made at the normal size of the shared cache");
    assert_eq!(made.map(|b| (b.width, b.height)), Some((96, 48)));
    let stored = cache.join("normal").join(crate::thumb::freedesktop_name(&book));
    assert_eq!(std::fs::metadata(&stored).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(cache.join("normal")).unwrap().permissions().mode() & 0o777, 0o700);
    let again = crate::thumb::thumbnail(&book, 96, Some(cache.clone())).expect("read back from the cache without running the thumbnailer");
    assert_eq!((again.width, again.height), (96, 48));
    produce(&book, 200, Some(&cache), |px| Some(png_data(px, px)));
    assert!(cache.join("large").join(crate::thumb::freedesktop_name(&book)).exists());
    assert_eq!(std::fs::read_dir(cache.join("normal")).unwrap().count(), 1, "no temporary file is left");
}

#[test]
fn a_changed_file_broken_output_or_a_preview_never_reuse_or_pollute_the_cache() {
    let t = Temp::new("thumb-cache-negative");
    let cache = t.path().join("thumbnails");
    let doc = t.file("report.odt", 9);
    produce(&doc, 96, Some(&cache), |px| Some(png_data(px, px)));
    std::fs::write(&doc, b"changed contents").unwrap();
    let older = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(&doc).unwrap().set_modified(older).unwrap();
    assert!(crate::thumb::thumbnail(&doc, 96, Some(cache.clone())).is_none(), "a thumbnail of the old contents is not shown");
    let other = t.file("broken.odt", 3);
    assert!(produce(&other, 96, Some(&cache), |_| Some(b"not a png".to_vec())).is_none());
    assert!(!cache.join("normal").join(crate::thumb::freedesktop_name(&other)).exists());
    let preview = t.file("preview.odt", 3);
    let mut asked = 0;
    assert!(produce(&preview, 700, None, |px| {
        asked = px;
        Some(png_data(px, px))
    })
    .is_some());
    assert_eq!(asked, 700, "without a cache the picture is made at the size asked");
    assert!(!cache.join("xx-large").exists());
    assert_eq!(store(&cache, &cache.join("normal").join("x.png"), "normal", &png_data(4, 4)), None, "thumbnails of thumbnails are not stored");
}
