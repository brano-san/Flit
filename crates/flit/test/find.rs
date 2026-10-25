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

fn median_ms(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
#[ignore]
fn bench_live_walk() {
    use crate::search_client::SearchClient;
    use flit_core::ui::{SearchEvent, SearchRequest, SearchScope, SearchSource};
    let files: Vec<String> = (0..400).flat_map(|d| (0..50).map(move |f| format!("d{}/s{d}/f{f:02}-{d}.txt", d / 20))).collect();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let dir = Scratch::new("findbench", &refs);
    let root = dir.path();
    for text in ["", "f07", "ext:md"] {
        let times: Vec<f64> = (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                let f = run(&root, text, &|| true);
                assert_eq!(f.folders, 421);
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        let hits = run(&root, text, &|| true).hits.len();
        eprintln!("BENCH walk 20 000 files in 421 folders for {text:?}: {hits} hits, median {:.1} ms", median_ms(times));
    }
    let windows = FsPath::parse(r"C:\Windows\System32").unwrap();
    for pass in ["cold", "warm"] {
        let t = std::time::Instant::now();
        let f = run(&windows, "kernel32", &|| true);
        eprintln!("BENCH walk C:\\Windows\\System32 ({pass}) for \"kernel32\": {} hits in {} folders, {:.0} ms", f.hits.len(), f.folders, t.elapsed().as_secs_f64() * 1e3);
    }
    let mut client = SearchClient::start::<crate::P>(std::sync::Arc::new(|| {}), 0, flit_core::tags::TagCell::default()).unwrap();
    let mut gen = 0;
    for (label, folder, text) in [("20 000 files", root.clone(), "f07"), (r"C:\Windows", FsPath::parse(r"C:\Windows").unwrap(), "kernel32")] {
        let folder = flit_core::Location::from_local(folder);
        let (mut firsts, mut dones, mut hits) = (Vec::new(), Vec::new(), 0);
        for _ in 0..5 {
            gen += 1;
            let t = std::time::Instant::now();
            client.submit(1, SearchRequest::Query { gen, text: text.into(), sort: SearchSort::default(), scope: SearchScope::Walk(folder.clone()) });
            let mut first = None;
            loop {
                let (_, ev) = client.events.recv_timeout(std::time::Duration::from_secs(60)).unwrap();
                if let SearchEvent::Found { gen: g, total, done, source: SearchSource::Walk, .. } = ev {
                    if g != gen {
                        continue;
                    }
                    if total > 0 {
                        first.get_or_insert(t.elapsed().as_secs_f64() * 1e3);
                    }
                    if done {
                        hits = total;
                        break;
                    }
                }
            }
            firsts.push(first.unwrap_or(f64::NAN));
            dones.push(t.elapsed().as_secs_f64() * 1e3);
        }
        eprintln!("BENCH folder search of {label} for {text:?} through the search thread: {hits} hits, first results median {:.1} ms, finished median {:.1} ms", median_ms(firsts), median_ms(dones));
    }
}
