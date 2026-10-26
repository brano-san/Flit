use super::*;
use flit_core::provider::{FileOp, Resolve};
use flit_platform::{FileOps, Job, Shell};
use std::time::Duration;

#[test]
fn recent_files_are_newest_first_unique_and_limited() {
    let root = std::env::temp_dir().join(format!("flit-recent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for d in ["links", "work"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    for f in ["old.txt", "new.txt"] {
        std::fs::write(root.join(f), "x").unwrap();
    }
    let at = |rel: &str| FsPath::parse(root.join(rel).to_str().unwrap()).unwrap();
    let mut sh = crate::ShellCom::new().unwrap();
    let out = sh.file_op(&FileOp::Link { items: vec![at("old.txt"), at("work"), at("new.txt")], dest: at("links") }, &Job::detached(Resolve::Skip));
    let links: Vec<FsPath> = out.items.into_iter().map(|(_, l)| l).collect();
    let base = SystemTime::now() - Duration::from_secs(3600);
    for (k, l) in links.iter().enumerate() {
        std::fs::File::options().write(true).open(&*l.display()).unwrap().set_modified(base + Duration::from_secs(60 * k as u64)).unwrap();
    }
    std::fs::copy(&*links[0].display(), root.join(r"links\again.lnk")).unwrap();
    std::fs::File::options().write(true).open(root.join(r"links\again.lnk")).unwrap().set_modified(base - Duration::from_secs(60)).unwrap();
    std::fs::write(root.join(r"links\note.txt"), "not a link").unwrap();
    let got = recent_files(&root.join("links"), 8);
    let shown: Vec<(&str, bool)> = got.iter().map(|r| (r.name.as_str(), r.dir)).collect();
    assert_eq!(shown, [("new.txt", false), ("work", true), ("old.txt", false)]);
    assert_eq!(got[1].target, at("work"));
    assert!(got[0].at > got[1].at && got[1].at > got[2].at);
    assert_eq!(recent_files(&root.join("links"), 1).len(), 1);
    assert!(recent_files(&root.join("missing"), 8).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn maps_windows_ui_languages_to_flit_codes() {
    assert_eq!([0x0419, 0x0409, 0x0809, 0x0407, 0x040C, 0x0C0A, 0x0422, 0x0804, 0x1004].map(language_code), ["ru", "en", "en", "de", "fr", "es", "uk", "zh-CN", "zh-CN"]);
    assert_eq!([0x0404, 0x0C04, 0x0411].map(language_code), ["", "", ""]);
}

#[test]
fn drives_end_with_the_network_place_and_mapped_drives_carry_their_share() {
    let all = drives();
    assert_eq!(all.last().map(|d| d.path.clone()), Some(FsPath::network()));
    assert!(all.iter().filter(|d| d.remote.is_some()).all(|d| d.path.kind() == flit_core::path::PathKind::Drive && d.label.contains('\\')));
}
