use super::*;
use flit_core::i18n::S;
use flit_core::ui::{Drive, Recent};
use std::time::{Duration, Instant};

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> Temp {
    let d = std::env::temp_dir().join(format!("flit-places-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Temp(d)
}

fn path(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn places() -> Places {
    Places {
        folders: vec![(KnownFolder::Downloads, Location::from_local(path(r"C:\Users\me\Downloads")))],
        drives: vec![Drive { path: FsPath::network(), label: "Network".into(), total: 0, free: 0, removable: false, unmounted: false, kind: Some(S::PathNetwork), remote: None }],
        wsl: vec!["Ubuntu".into()],
        recycle_bin: true,
        recent: vec![Recent { name: "notes.txt".into(), target: path(r"C:\notes.txt"), dir: false, at: 7 }],
        ..Default::default()
    }
}

fn request(key: u64, gen: u64, kind: IconKind) -> IconRequest {
    IconRequest { key, kind, path: path(r"C:\Windows"), size: 20, gen, row: 0 }
}

fn icon(seed: u8) -> Bitmap {
    Bitmap { width: 20, height: 20, rgba: vec![seed; 20 * 20 * 4] }
}

#[test]
fn the_sidebar_comes_back_from_the_last_run_before_anything_is_collected() {
    let d = temp("snapshot");
    let file = d.0.join(PLACES_FILE);
    store(&file, &encode(&places()).unwrap());
    assert_eq!(load::<Places>(&file), Some(places()));
    std::fs::write(&file, b"\x02garbage").unwrap();
    assert_eq!(load::<Places>(&file), None, "a snapshot from another version is ignored");
}

#[test]
fn only_window_chrome_icons_are_kept_and_they_survive_a_restart() {
    let d = temp("icons");
    let file = d.0.join(ICONS_FILE);
    let mut icons = ChromeIcons::load(Some(file.clone()));
    icons.keep(&request(1, 0, IconKind::Place), &icon(1));
    icons.keep(&request(2, 5, IconKind::Type), &icon(2));
    icons.keep(&request(3, 0, IconKind::Thumbnail), &icon(3));
    icons.keep(&request(1, 0, IconKind::Place), &icon(9));
    icons.save();
    let back = ChromeIcons::load(Some(file));
    assert_eq!(back.kept.iter().map(|k| (k.key, k.image.rgba[0])).collect::<Vec<_>>(), vec![(1, 9)]);
    assert_eq!(back.kept[0].request().gen, 0);
}

#[test]
fn the_kept_icons_stay_bounded() {
    let mut icons = ChromeIcons::load(None);
    for k in 0..KEPT_ICONS as u64 + 10 {
        icons.keep(&request(k, 0, IconKind::Place), &icon(0));
    }
    assert_eq!(icons.kept.len(), KEPT_ICONS);
    assert_eq!(icons.kept[0].key, 10, "the oldest go first");
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn loading_the_kept_sidebar_fits_in_a_frame() {
    let d = temp("budget");
    let mut icons = ChromeIcons::load(Some(d.0.join(ICONS_FILE)));
    for k in 0..KEPT_ICONS as u64 {
        icons.keep(&request(k, 0, IconKind::Place), &icon(k as u8));
    }
    icons.save();
    let file = d.0.join(PLACES_FILE);
    store(&file, &encode(&places()).unwrap());
    let t = Instant::now();
    let back = ChromeIcons::load(Some(d.0.join(ICONS_FILE)));
    let snapshot = load::<Places>(&file);
    let took = t.elapsed();
    assert_eq!((back.kept.len(), snapshot.is_some()), (KEPT_ICONS, true));
    assert!(took < Duration::from_millis(8), "{took:?}");
}
