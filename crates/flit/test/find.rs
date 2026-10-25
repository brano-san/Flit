use super::*;
use flit_core::ui::{SearchKey, SearchSort};
use flit_platform::Platform;
use flit_search::query::{parse, Clock};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct Scratch(pub PathBuf);

impl Scratch {
    pub(crate) fn new(tag: &str, files: &[&str]) -> Self {
        let root = std::env::temp_dir().join(format!("flit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in files {
            let p = root.join(f);
            if f.ends_with('/') {
                std::fs::create_dir_all(&p).unwrap();
            } else {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, f.as_bytes()).unwrap();
            }
        }
        Self(root)
    }

    pub(crate) fn path(&self) -> FsPath {
        FsPath::parse(&self.0.to_string_lossy()).unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn names(found: &Found) -> Vec<String> {
    found.hits.iter().map(|h| String::from_utf8_lossy(&h.row.name).into_owned()).collect()
}

fn run(root: &FsPath, text: &str, keep_going: KeepGoing<'_>) -> Found {
    let q = parse(text, Clock::system(0)).unwrap();
    let found = Mutex::new(Found::default());
    walk(&crate::P::fs(), root, &q, keep_going, &found);
    let mut f = found.into_inner().unwrap();
    sort(&mut f.hits, SearchSort { key: SearchKey::Folder, descending: false });
    f
}

#[test]
fn the_walk_matches_names_and_paths_in_every_subfolder() {
    let dir = Scratch::new("find", &["report.txt", "a/report-2.txt", "a/b/notes.md", "a/b/c/report.md", "empty/"]);
    let root = dir.path();
    let f = run(&root, "report", &|| true);
    assert_eq!(names(&f), ["report.txt", "report-2.txt", "report.md"]);
    assert_eq!(f.folders, 5, "the root, a, a\\b, a\\b\\c and empty");
    let top = root.as_bytes().to_vec();
    assert_eq!(f.hits[0].row.dir, top);
    assert_eq!(groups(&f.hits), [0, 1, 2]);
    assert_eq!(names(&run(&root, "ext:md", &|| true)), ["notes.md", "report.md"]);
    assert_eq!(names(&run(&root, r"path:\b\", &|| true)), ["c", "notes.md", "report.md"]);
    let stop = AtomicBool::new(false);
    let f = run(&root, "", &|| !stop.swap(true, Ordering::Relaxed));
    assert!(f.folders <= 1, "a cancelled walk stops at once");
}

#[test]
fn hits_know_whether_they_are_inside_a_folder() {
    let hit = |dir: &str| Hit::new(SearchRow { name: b"x".to_vec(), dir: dir.as_bytes().to_vec(), meta: Default::default() });
    assert!(hit(r"C:\Data").within(br"c:\data"));
    assert!(hit(r"C:\Data\Sub").within(br"c:\data"));
    assert!(!hit(r"C:\Database").within(br"c:\data"));
    assert!(hit(r"C:\Data").within(br"c:\"));
}
