use super::*;
use crate::index::{IndexRecord, IndexSink};
use crate::query::{parse, Clock};
use flit_core::attr;

const DAY: i64 = 86_400 * 10_000_000;

fn clock() -> Clock {
    Clock { now: 400 * DAY, utc_offset_min: 0 }
}

fn put(v: &mut VolumeIndex, id: u64, parent: u64, name: &str, size: u64, age_days: i64, dir: bool) {
    let attrs = if dir { attr::DIR } else { 0 };
    v.upsert(&IndexRecord { id, parent, name: name.as_bytes(), size, modified: (400 - age_days) * DAY, attrs });
}

fn sample() -> Engine {
    let mut c = VolumeIndex::new("C:", b"C:\\", b'\\');
    put(&mut c, 5, 5, ".", 0, 0, true);
    put(&mut c, 10, 5, "Projects", 0, 9, true);
    put(&mut c, 11, 10, "src", 0, 9, true);
    put(&mut c, 12, 11, "main.rs", 1200, 0, false);
    put(&mut c, 13, 11, "Report.TXT", 5 << 20, 1, false);
    put(&mut c, 14, 10, "report-old.txt", 10, 30, false);
    put(&mut c, 15, 5, "notes", 300, 2, false);
    c.commit();
    let mut d = VolumeIndex::new("D:", b"D:\\", b'\\');
    put(&mut d, 5, 5, ".", 0, 0, true);
    put(&mut d, 20, 5, "backup", 0, 3, true);
    put(&mut d, 21, 20, "report.bak", 700, 5, false);
    d.commit();
    ready(Engine { volumes: vec![c, d], version: 1 })
}

fn names(e: &Engine, text: &str, key: SortKey, descending: bool) -> Vec<String> {
    let q = parse(text, clock()).unwrap();
    let mut rs = e.search(&q, Sort { key, descending }, &|| false).unwrap();
    let (batch, dirs) = e.rows(&mut rs, 0, 100);
    assert_eq!(batch.len() as u64, rs.total, "{text}");
    batch
        .entries(true)
        .unwrap()
        .zip(dirs)
        .map(|((n, _), d)| {
            let d = String::from_utf8(d).unwrap();
            let sep = if d.ends_with('\\') { "" } else { "\\" };
            format!("{d}{sep}{}", String::from_utf8_lossy(n))
        })
        .collect()
}

#[test]
fn filters_and_sorts_across_volumes() {
    let e = sample();
    assert_eq!(names(&e, "report", SortKey::Name, false), [r"C:\Projects\report-old.txt", r"D:\backup\report.bak", r"C:\Projects\src\Report.TXT"]);
    assert_eq!(names(&e, "report", SortKey::Size, false), [r"C:\Projects\report-old.txt", r"D:\backup\report.bak", r"C:\Projects\src\Report.TXT"]);
    assert_eq!(names(&e, "report", SortKey::Size, true), [r"C:\Projects\src\Report.TXT", r"D:\backup\report.bak", r"C:\Projects\report-old.txt"]);
    assert_eq!(names(&e, "report", SortKey::Modified, true), [r"C:\Projects\src\Report.TXT", r"D:\backup\report.bak", r"C:\Projects\report-old.txt"]);
    assert_eq!(names(&e, "report", SortKey::Path, false), [r"C:\Projects\report-old.txt", r"C:\Projects\src\Report.TXT", r"D:\backup\report.bak"]);
    assert_eq!(names(&e, "report", SortKey::Path, true), [r"D:\backup\report.bak", r"C:\Projects\src\Report.TXT", r"C:\Projects\report-old.txt"]);
    assert_eq!(names(&e, "", SortKey::Path, false).len(), 8);
    assert_eq!(names(&e, "", SortKey::Path, false)[..2], [r"C:\notes", r"C:\Projects"]);
    assert_eq!(names(&e, "rep !old ext:txt;bak", SortKey::Name, false), [r"D:\backup\report.bak", r"C:\Projects\src\Report.TXT"]);
    assert_eq!(names(&e, "size:>1kb", SortKey::Name, false), [r"C:\Projects\src\main.rs", r"C:\Projects\src\Report.TXT"]);
    assert_eq!(names(&e, "dm:>=2100", SortKey::Name, false).len(), 0);
    assert_eq!(names(&e, "dm:1602", SortKey::Name, false).len(), 8);
    assert_eq!(names(&e, "*.txt", SortKey::Name, false), [r"C:\Projects\report-old.txt", r"C:\Projects\src\Report.TXT"]);
    assert_eq!(names(&e, "path:projects\\src", SortKey::Name, false), [r"C:\Projects\src\main.rs", r"C:\Projects\src\Report.TXT", r"C:\Projects\src"]);
    assert_eq!(names(&e, "src\\m", SortKey::Name, false), [r"C:\Projects\src\main.rs"]);
    assert_eq!(names(&e, "path:regex:^d:.*\\.bak$", SortKey::Name, false), [r"D:\backup\report.bak"]);
    assert!(names(&e, "report tag:Work", SortKey::Name, false).is_empty(), "the index knows no tags");
    assert_eq!(names(&e, "report NOT tag:Work", SortKey::Name, false).len(), 3);
    assert_eq!(names(&e, r"regex:^r\w+\.txt$", SortKey::Name, false), [r"C:\Projects\src\Report.TXT"]);
    assert_eq!(names(&e, "size:>0 notes", SortKey::Name, false), [r"C:\notes"]);
    assert!(names(&e, "ext:", SortKey::Name, false).contains(&r"C:\notes".to_string()));
    assert!(!names(&e, "size:0", SortKey::Name, false).contains(&r"C:\Projects".to_string()), "folders have no size");
}

#[test]
fn w3_review_exclusions_keep_the_permutation_and_filter_by_ancestor_and_exact_file() {
    let e = sample();
    let sort = Sort { key: SortKey::Name, descending: true };
    let mut q = parse("", clock()).unwrap();
    q.paths.exclude = [r"c:\PROJECTS", r"D:\backup\report.bak", r"D:\backup\not-found", r"Q:\"].map(|p| FsPath::parse(p).unwrap()).to_vec();
    let mut set = e.search(&q, sort, &|| false).unwrap();
    assert!(set.streams.iter().all(|s| matches!(s.kind, Kind::Perm(Some(_)))));
    let (batch, _) = e.rows(&mut set, 0, 10);
    assert_eq!(set.total, 2);
    assert_eq!(batch.entries(true).unwrap().map(|(n, _)| n).collect::<Vec<_>>(), [b"notes".as_slice(), b"backup"]);

    q.terms = parse("report", clock()).unwrap().terms;
    q.paths.exclude.pop();
    q.paths.exclude.remove(1);
    let mut set = e.search(&q, sort, &|| false).unwrap();
    let (batch, _) = e.rows(&mut set, 0, 10);
    assert_eq!(batch.entries(true).unwrap().map(|(n, _)| n).collect::<Vec<_>>(), [b"report.bak".as_slice()]);
    q.paths.parent = Some(FsPath::parse(r"d:\BACKUP").unwrap());
    assert_eq!(e.search(&q, sort, &|| false).unwrap().total, 1);
    q.paths.parent = Some(FsPath::parse(r"D:\").unwrap());
    assert_eq!(e.search(&q, sort, &|| false).unwrap().total, 0);

    let mut q = parse("", clock()).unwrap();
    q.paths.exclude = vec![FsPath::parse(r"C:\not-found").unwrap(), FsPath::parse(r"D:\").unwrap()];
    let set = e.search(&q, sort, &|| false).unwrap();
    assert!(matches!(&set.streams[..], [Stream { kind: Kind::Perm(None), .. }]));
    assert_eq!(set.total, 6);
}

#[test]
fn w3_review_indexed_folder_scope_uses_components_on_unix_mounts() {
    let mut v = VolumeIndex::new("/mnt/data", b"/mnt/data", b'/');
    put(&mut v, 1, 1, ".", 0, 0, true);
    put(&mut v, 2, 1, "Docs", 0, 0, true);
    put(&mut v, 3, 1, "Docs-other", 0, 0, true);
    put(&mut v, 4, 2, "axb", 5, 0, false);
    put(&mut v, 5, 3, "axb", 5, 0, false);
    put(&mut v, 6, 2, "other", 5, 0, false);
    v.commit();
    let e = Engine { volumes: vec![v], version: 1 };
    let mut q = parse("file: < other OR regex:^a.*b$ >", clock()).unwrap();
    let sort = Sort { key: SortKey::Name, descending: false };
    let search = |q: &Query| e.search(q, sort, &|| false).unwrap();
    q.paths.under = Some(FsPath::parse("/mnt/data/Docs").unwrap());
    let mut set = search(&q);
    let (_, dirs) = e.rows(&mut set, 0, 10);
    assert_eq!(dirs, vec![b"/mnt/data/Docs".to_vec(); 2]);
    q.paths.under = Some(FsPath::parse("/mnt/data/docs").unwrap());
    assert_eq!(search(&q).total, 0);
    q.paths.under = Some(FsPath::parse("/mnt").unwrap());
    assert_eq!(search(&q).total, 3);
    q.paths.exclude.push(FsPath::parse("/mnt").unwrap());
    assert_eq!(search(&q).total, 0);
}

#[test]
fn w3_review_child_count_and_empty_share_the_column_until_the_index_changes() {
    use std::sync::atomic::AtomicBool;
    let mut e = sample();
    let cancelled = AtomicBool::new(false);
    assert!(e.volumes[0].child_counts(&|| cancelled.swap(true, Relaxed)).is_none());
    assert_eq!(names(&e, "child-count:2", SortKey::Name, false).len(), 2);
    let cached = e.volumes[0].clone();
    let counts = cached.child_counts(&|| panic!("completed column should be cached")).unwrap();
    assert!(std::sync::Arc::ptr_eq(&e.volumes[0].child_counts(&|| panic!("completed column should be cached")).unwrap(), &counts));
    assert!(names(&e, "empty:true", SortKey::Name, false).is_empty());
    for id in [12, 13] {
        e.volumes[0].remove(id);
    }
    e.volumes[0].commit();
    e.version += 1;
    assert_eq!(names(&e, "empty:true", SortKey::Name, false), [r"C:\Projects\src"]);
    assert_eq!(counts[cached.lookup(11).unwrap() as usize], 2);
    assert_eq!(e.volumes[0].child_counts(&|| false).unwrap()[e.volumes[0].lookup(11).unwrap() as usize], 0);
}

#[test]
fn dates_filter_on_modified_time() {
    let e = sample();
    let mut c = clock();
    c.now += DAY / 2;
    let q = parse("dm:today", c).unwrap();
    let mut rs = e.search(&q, Sort { key: SortKey::Name, descending: false }, &|| false).unwrap();
    let (batch, _) = e.rows(&mut rs, 0, 10);
    let got: Vec<&[u8]> = batch.entries(true).unwrap().map(|(n, _)| n).collect();
    assert_eq!(got, [b"main.rs".as_slice()]);
}

#[test]
fn rows_carry_metadata_and_respect_the_window() {
    let e = sample();
    let q = parse("", clock()).unwrap();
    let mut rs = e.search(&q, Sort { key: SortKey::Size, descending: true }, &|| false).unwrap();
    assert_eq!(rs.total, 8);
    let (batch, dirs) = e.rows(&mut rs, 0, 1);
    let (name, meta) = batch.entries(true).unwrap().next().unwrap();
    assert_eq!((name, meta.size, meta.attrs), (b"Report.TXT".as_slice(), 5 << 20, 0));
    assert_eq!(dirs, [br"C:\Projects\src".to_vec()]);
    let (batch, _) = e.rows(&mut rs, 6, 100);
    assert_eq!(batch.len(), 2);
    assert!(batch.entries(true).unwrap().all(|(_, m)| m.attrs == attr::DIR));
    assert!(e.rows(&mut rs, 8, 100).0.is_empty());
    assert!(e.rows(&mut rs, 50, 40).0.is_empty());
}

#[test]
fn cancelled_search_returns_nothing() {
    let e = sample();
    let q = parse("report", clock()).unwrap();
    assert!(e.search(&q, Sort { key: SortKey::Name, descending: false }, &|| true).is_none());
}

#[test]
fn large_result_sets_walk_the_permutation() {
    let mut v = VolumeIndex::new("C:", b"C:\\", b'\\');
    put(&mut v, 5, 5, ".", 0, 0, true);
    let n = SMALL as u64 + 5000;
    for k in 0..n {
        put(&mut v, 100 + k, 5, &format!("item{}", (k * 7919) % n), k % 1000, 0, false);
    }
    put(&mut v, 99, 5, "other", 0, 0, false);
    v.commit();
    let e = ready(Engine { volumes: vec![v], version: 1 });
    let q = parse("item", clock()).unwrap();
    for key in [SortKey::Name, SortKey::Size] {
        for descending in [false, true] {
            let mut rs = e.search(&q, Sort { key, descending }, &|| false).unwrap();
            assert_eq!(rs.total, n);
            let mut seen = Vec::new();
            let mut from = 0;
            while from < n {
                let (batch, _) = e.rows(&mut rs, from, from + MAX_ROWS);
                from += batch.len() as u64;
                seen.extend(batch.entries(true).unwrap().map(|(name, m)| (name.to_vec(), m.size)));
            }
            assert_eq!(seen.len() as u64, n);
            let mut sorted = seen.clone();
            match key {
                SortKey::Size => sorted.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0))),
                _ => sorted.sort(),
            }
            if descending {
                sorted.reverse();
            }
            assert_eq!(seen, sorted, "{key:?} {descending}");
        }
    }
}

struct Deny(&'static str);

impl Visibility for Deny {
    fn can_list(&self, v: &VolumeIndex, dir: u32) -> bool {
        dir == crate::index::NONE || v.name(dir) != self.0.as_bytes()
    }
}

#[test]
fn visibility_hides_entries_of_unlistable_folders() {
    let e = sample();
    let sort = Sort { key: SortKey::Name, descending: false };
    let deny = Deny("src");
    let q = parse("report", clock()).unwrap();
    let mut rs = e.search_as(&q, sort, Some(&deny), &|| false).unwrap();
    assert_eq!(rs.total, 2, "Report.TXT lives in the hidden folder");
    let (batch, _) = e.rows(&mut rs, 0, 10);
    let got: Vec<Vec<u8>> = batch.entries(true).unwrap().map(|(n, _)| n.to_vec()).collect();
    assert_eq!(got, [b"report-old.txt".to_vec(), b"report.bak".to_vec()]);
    let all = parse("", clock()).unwrap();
    let everything = e.search(&all, sort, &|| false).unwrap().total;
    let visible = e.search_as(&all, sort, Some(&deny), &|| false).unwrap().total;
    assert_eq!((everything, visible), (8, 6), "the empty query is filtered too");
    assert_eq!(e.search_as(&all, sort, Some(&Deny("nothing")), &|| false).unwrap().total, everything);
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn bench_folder_scoped_queries_and_groups() {
    let mut v = VolumeIndex::new("C:", b"C:\\", b'\\');
    put(&mut v, 5, 5, ".", 0, 0, true);
    let (tops, subs, files) = (100u64, 100u64, 100u64);
    for t in 0..tops {
        put(&mut v, 10 + t, 5, &format!("top {t}"), 0, 1, true);
        for s in 0..subs {
            let sid = 1_000 + t * subs + s;
            put(&mut v, sid, 10 + t, &format!("sub {s}"), 0, 1, true);
            for f in 0..files {
                let name = if f % 25 == 0 { format!("report {f}.txt") } else { format!("file {f}.dat") };
                put(&mut v, 10_000_000 + sid * files + f, sid, &name, f, 1, false);
            }
        }
    }
    v.commit();
    let e = ready(Engine { volumes: vec![v], version: 1 });
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
    let median = |text: &str, key: SortKey| {
        let q = parse(text, clock()).unwrap();
        let mut times = Vec::new();
        let mut total = 0;
        for _ in 0..7 {
            let t = std::time::Instant::now();
            let mut rs = e.search(&q, Sort { key, descending: false }, &|| false).unwrap();
            let _ = e.rows(&mut rs, 0, 64);
            total = rs.total;
            times.push(ms(t));
        }
        times.sort_by(f64::total_cmp);
        (total, times[3])
    };
    eprintln!("BENCH synthetic index of {} entries", e.total());
    for text in ["report", r#"report path:"C:\top 42\""#, r#"path:"C:\top 42\""#, r#"path:"C:\top 42\sub 7\""#] {
        let (total, t) = median(text, SortKey::Name);
        eprintln!("BENCH {text:?}: {total} results, query and first page {t:.1} ms");
        assert!(t < 20.0, "{text:?} took {t:.1} ms");
    }
    let q = parse(r#"path:"C:\top 42\""#, clock()).unwrap();
    let mut rs = e.search(&q, Sort { key: SortKey::Path, descending: false }, &|| false).unwrap();
    let t = std::time::Instant::now();
    let starts = e.groups(&mut rs).unwrap();
    eprintln!("BENCH folder groups of {} results: {} groups in {:.1} ms", rs.total, starts.len(), ms(t));
    assert!(ms(t) < 10.0, "groups took {:.1} ms", ms(t));
}

#[test]
fn every_scan_path_agrees_with_the_plain_matcher() {
    let mut v = VolumeIndex::new("C:", b"C:\\", b'\\');
    put(&mut v, 5, 5, ".", 0, 0, true);
    let stems = ["report", "Report-1", "rep", ".rs", "a.", "noext", "x.tar.gz", "main.rs", "lib.RS", "Документ", "img_0042.jpg", "h"];
    let n = SMALL as u64 + 3000;
    for k in 0..n {
        let name = format!("{}{}{}", stems[(k % 12) as usize], k % 97, if k % 5 == 0 { ".h" } else { "" });
        put(&mut v, 100 + k, 5, &name, (k * 7919) % 5000, (k % 50) as i64, k % 11 == 0);
    }
    v.commit();
    let e = ready(Engine { volumes: vec![v], version: 1 });
    let v = &e.volumes[0];
    for text in [".", "r", "rep", "report-1", "докум", "ext:rs;h", "ext:", "ext:gz", "!ext:h", "size:>1k", "!size:<2k", "!dm:today", "rep size:<3k", "*.jpg", "img* !size:>4k", "regex:^rep\\d"] {
        let q = parse(text, clock()).unwrap();
        let expected: Vec<u32> = (0..v.len() as u32)
            .filter(|&i| v.listed(i))
            .filter(|&i| {
                let plain = crate::query::Plain { name: v.name(i), lower_name: v.lower_name(i), path: b"", lower_path: b"", size: v.sizes[i as usize], modified: v.modified[i as usize], dir: v.is_dir(i) };
                q.matches(&plain, &|_| false)
            })
            .collect();
        for key in [SortKey::Name, SortKey::Size] {
            for descending in [false, true] {
                let mut rs = e.search(&q, Sort { key, descending }, &|| false).unwrap();
                assert_eq!(rs.total, expected.len() as u64, "{text}");
                rs.ensure(&e, expected.len());
                let mut got: Vec<u32> = rs.rows.iter().map(|&(_, i)| i).collect();
                let rank = &v.ranks[order_of(key) as usize];
                assert!(got.windows(2).all(|w| (rank[w[0] as usize] < rank[w[1] as usize]) != descending), "{text} {key:?} {descending} order");
                got.sort_unstable();
                assert_eq!(got, expected, "{text} {key:?} {descending}");
            }
        }
    }
}

#[test]
fn path_text_terms_agree_with_matching_the_whole_path() {
    let mut v = VolumeIndex::new("C:", b"C:\\", b'\\');
    put(&mut v, 5, 5, ".", 0, 0, true);
    let words = ["src", "Ab", "b", "srcab", "x", "Документ", "a b"];
    let mut id = 10u64;
    let mut dirs = vec![5u64];
    for k in 0..400u64 {
        let parent = dirs[(k * 7919 % dirs.len() as u64) as usize];
        let dir = k % 3 != 2;
        put(&mut v, id, parent, words[(k % 7) as usize], k, 1, dir);
        if dir {
            dirs.push(id);
        }
        id += 1;
    }
    put(&mut v, id, 99_999, "orphan src", 0, 1, false);
    v.commit();
    let e = ready(Engine { volumes: vec![v], version: 1 });
    let v = &e.volumes[0];
    for text in ["path:src", "path:c:\\src", "path:\\ab\\", "path:b\\s", "path:rcab\\x", "!path:src", "path:документ\\a", "path:a b\\b", "path:\\", "path:c:\\", "path:src path:\\x", "path:zzz", "path:ab\\b\\src", "path:\\b", "path:c:\\ab\\", "!path:b\\", "path:src\\"]
    {
        let q = parse(text, clock()).unwrap();
        let expected: Vec<u32> = (0..v.len() as u32)
            .filter(|&i| v.listed(i))
            .filter(|&i| {
                let (mut path, mut lower_path) = (Vec::new(), Vec::new());
                v.path_into(i, false, &mut Vec::new(), &mut path);
                v.path_into(i, true, &mut Vec::new(), &mut lower_path);
                let plain = crate::query::Plain { name: v.name(i), lower_name: v.lower_name(i), path: &path, lower_path: &lower_path, size: v.sizes[i as usize], modified: v.modified[i as usize], dir: v.is_dir(i) };
                q.matches(&plain, &|_| false)
            })
            .collect();
        let mut rs = e.search(&q, Sort { key: SortKey::Name, descending: false }, &|| false).unwrap();
        rs.ensure(&e, expected.len());
        let mut got: Vec<u32> = rs.rows.iter().map(|&(_, i)| i).collect();
        got.sort_unstable();
        assert_eq!(got, expected, "{text}");
    }
}

fn ready(mut e: Engine) -> Engine {
    for order in [Order::Size, Order::Modified, Order::Path] {
        e.build_order(order);
    }
    e
}
