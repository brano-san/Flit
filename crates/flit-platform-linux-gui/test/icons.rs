use super::*;
use crate::temp::Temp;
use std::time::Instant;

const INDEX: &str = "[Icon Theme]\nName=Mine\nInherits=Base\nDirectories=16x16/mimetypes,48x48/mimetypes,scalable/places,48x48@2/mimetypes\n\n[16x16/mimetypes]\nSize=16\nType=Fixed\n\n[48x48/mimetypes]\nSize=48\nType=Threshold\n\n[scalable/places]\nSize=64\nMinSize=8\nMaxSize=512\nType=Scalable\n\n[48x48@2/mimetypes]\nSize=48\nScale=2\nType=Fixed\n";
const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#00ff00"/></svg>"##;

fn png(path: &Path, side: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), side, side);
    e.set_color(png::ColorType::Rgba);
    e.write_header().unwrap().write_image_data(&vec![255u8; (side * side * 4) as usize]).unwrap();
}

fn fixture(t: &Temp) -> PathBuf {
    let base = t.path().join("icons");
    std::fs::create_dir_all(base.join("Mine/scalable/places")).unwrap();
    std::fs::write(base.join("Mine/index.theme"), INDEX).unwrap();
    png(&base.join("Mine/16x16/mimetypes/text-x-rust.png"), 16);
    png(&base.join("Mine/48x48/mimetypes/text-x-rust.png"), 48);
    std::fs::write(base.join("Mine/scalable/places/folder.svg"), SVG).unwrap();
    std::fs::create_dir_all(base.join("Base")).unwrap();
    std::fs::write(base.join("Base/index.theme"), "[Icon Theme]\nDirectories=32x32/devices\n[32x32/devices]\nSize=32\nType=Fixed\n").unwrap();
    png(&base.join("Base/32x32/devices/drive-harddisk.png"), 32);
    base
}

#[test]
fn theme_settings_are_read_from_ini_files() {
    assert_eq!(setting("[Settings]\ngtk-icon-theme-name = \"Papirus\"\n", "gtk-icon-theme-name").as_deref(), Some("Papirus"));
    assert_eq!(setting("[Icons]\nTheme=breeze\n", "Theme").as_deref(), Some("breeze"));
    assert_eq!(setting("Theme=\n", "Theme"), None);
}

#[test]
fn the_index_lists_sized_and_scalable_directories() {
    let t = parse_index(INDEX);
    assert_eq!(t.inherits, ["Base"]);
    assert_eq!(t.dirs.len(), 4);
    assert_eq!((t.dirs[2].kind, t.dirs[2].min, t.dirs[2].max), (Kind::Scalable, 8, 512));
    assert_eq!(t.dirs[3].scale, 2);
    assert!(t.dirs[1].matches(50) && !t.dirs[1].matches(51));
}

#[test]
fn lookups_take_the_exact_size_then_the_nearest_then_inherited_themes() {
    let t = Temp::new("icons");
    let base = fixture(&t);
    let themes = chain(std::slice::from_ref(&base), "Mine");
    assert_eq!(themes.len(), 2);
    let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let rust = |size| find(&themes, &names(&["text-x-rust"]), size).unwrap();
    assert!(rust(16).ends_with("16x16/mimetypes/text-x-rust.png"));
    assert!(rust(48).ends_with("48x48/mimetypes/text-x-rust.png"));
    assert!(rust(24).ends_with("16x16/mimetypes/text-x-rust.png"), "{:?}", rust(24));
    assert!(find(&themes, &names(&["no-such", "drive-harddisk"]), 48).unwrap().ends_with("Base/32x32/devices/drive-harddisk.png"));
    assert!(find(&themes, &names(&["nothing"]), 48).is_none());
    if crate::svg::available() {
        let folder = render(&find(&themes, &names(&["folder"]), 48).unwrap(), 48).unwrap();
        assert_eq!((folder.width, folder.height), (48, 48));
        assert_eq!(&folder.rgba[..4], &[0, 255, 0, 255]);
    } else {
        assert!(find(&themes, &names(&["folder"]), 48).is_none(), "without librsvg only PNG icons are offered");
    }
    let drive = render(&find(&themes, &names(&["drive-harddisk"]), 24).unwrap(), 24).unwrap();
    assert_eq!((drive.width, drive.height), (24, 24));
}

#[test]
fn file_kinds_map_to_freedesktop_icon_names() {
    let req = |kind, path: &str| flit_core::ui::IconRequest { key: 0, kind, path: flit_core::FsPath::parse(path).unwrap(), size: 32, gen: 0, row: 0 };
    assert_eq!(crate::shell::icon_names(&req(flit_core::ui::IconKind::Volume, "/media/ann/USB")), ["drive-removable-media", "drive-harddisk"]);
    assert_eq!(crate::shell::icon_names(&req(flit_core::ui::IconKind::Folder, "/srv")), ["folder"]);
    let file = crate::shell::icon_names(&req(flit_core::ui::IconKind::Type, "/x/.bashrc"));
    assert_eq!(file, ["text-x-generic", "unknown"]);
    if std::path::Path::new("/usr/share/mime/globs2").exists() {
        let png = crate::shell::icon_names(&req(flit_core::ui::IconKind::Type, "/x/a.PNG"));
        assert_eq!(&png[..2], ["image-png", "image-x-generic"]);
    }
}

#[test]
#[ignore]
fn bench_an_icon_from_the_cache() {
    let names = vec!["folder".to_string(), "text-x-generic".to_string(), "image-x-generic".to_string()];
    let start = Instant::now();
    let first = icon(&names, 32);
    let cold = start.elapsed();
    let start = Instant::now();
    for _ in 0..1000 {
        assert_eq!(icon(&names, 32).is_some(), first.is_some());
    }
    eprintln!("measure: system theme icon cold {cold:?}, from the cache {:?} each, found {}", start.elapsed() / 1000, first.is_some());
}
