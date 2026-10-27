use super::*;

fn rec(id: u64, parent: u64, name: &str, dir: bool) -> IndexRecord<'_> {
    IndexRecord { id, parent, name: name.as_bytes(), size: id * 10, modified: id as i64, attrs: if dir { attr::DIR } else { 0 } }
}

fn volume() -> VolumeIndex {
    let mut v = VolumeIndex::new("C:", b"C:\\", b'\\');
    v.upsert(&rec(5, 5, ".", true));
    v
}

fn path(v: &VolumeIndex, id: u64) -> String {
    let mut out = Vec::new();
    v.path_into(v.lookup(id).unwrap(), false, &mut Vec::new(), &mut out);
    String::from_utf8(out).unwrap()
}

#[test]
fn lowercase_folds_unicode_and_keeps_invalid_bytes() {
    assert_eq!(lowercase("ÄÖ Straße ДОК".as_bytes()), "äö straße док".as_bytes());
    assert_eq!(lowercase(b"A\xFFB"), b"a\xFFb");
    assert_eq!(lowercase(&[b'X', 0xED, 0xA0, 0x80]), [b'x', 0xED, 0xA0, 0x80]);
}

#[test]
fn paths_follow_renames_and_moves() {
    let mut v = volume();
    v.upsert(&rec(10, 5, "Users", true));
    v.upsert(&rec(11, 10, "Ann", true));
    v.upsert(&rec(12, 11, "Notes.TXT", false));
    v.commit();
    assert_eq!(path(&v, 12), r"C:\Users\Ann\Notes.TXT");
    assert_eq!(v.lower_name(v.lookup(12).unwrap()), b"notes.txt");
    assert_eq!(v.ext(v.lookup(12).unwrap()), b"txt");
    assert!(!v.listed(v.lookup(5).unwrap()), "the root is never a result");
    v.upsert(&rec(11, 5, "Bob", true));
    v.commit();
    assert_eq!(path(&v, 12), r"C:\Bob\Notes.TXT");
    let mut dir = Vec::new();
    v.dir_into(v.lookup(12).unwrap(), &mut Vec::new(), &mut dir);
    assert_eq!(dir, br"C:\Bob");
    assert_eq!(v.live(), 3);
}

#[test]
fn orphans_resolve_when_the_parent_arrives_later() {
    let mut v = volume();
    v.upsert(&rec(21, 20, "late.txt", false));
    v.upsert(&rec(20, 5, "dir", true));
    v.commit();
    assert_eq!(path(&v, 21), r"C:\dir\late.txt");
}

#[test]
fn removing_a_directory_removes_its_subtree() {
    let mut v = volume();
    v.upsert(&rec(10, 5, "a", true));
    v.upsert(&rec(11, 10, "b", true));
    v.upsert(&rec(12, 11, "c.txt", false));
    v.upsert(&rec(13, 5, "keep.txt", false));
    v.commit();
    v.remove(10);
    v.commit();
    assert_eq!(v.live(), 1);
    assert!(v.lookup(12).is_none());
    assert_eq!(v.perms[Order::Name as usize].len(), 1);
    assert_eq!(v.name(v.perms[Order::Name as usize][0]), b"keep.txt");
}

#[test]
fn rename_reported_as_remove_then_add_keeps_children() {
    let mut v = volume();
    v.upsert(&rec(10, 5, "old", true));
    v.upsert(&rec(11, 10, "child.txt", false));
    v.commit();
    v.remove(10);
    v.upsert(&rec(10, 5, "new", true));
    v.commit();
    assert_eq!(path(&v, 11), r"C:\new\child.txt");
    assert_eq!(v.live(), 2);
}

fn brute(v: &VolumeIndex, order: Order) -> Vec<u32> {
    let mut all: Vec<u32> = (0..v.len() as u32).filter(|&i| v.listed(i)).collect();
    all.sort_by(|&a, &b| v.cmp_name(a, b));
    let rank: std::collections::HashMap<u32, usize> = all.iter().enumerate().map(|(r, &i)| (i, r)).collect();
    let chain = |i: u32| {
        let mut c = Vec::new();
        let mut p = v.parents[i as usize];
        while p != NONE && v.listed(p) {
            c.push(rank[&p]);
            p = v.parents[p as usize];
        }
        c.reverse();
        c
    };
    match order {
        Order::Name => {}
        Order::Size => all.sort_by_key(|&i| (v.sizes[i as usize], rank[&i])),
        Order::Modified => all.sort_by_key(|&i| (v.modified[i as usize], rank[&i])),
        Order::Path => all.sort_by_key(|&i| (chain(i), rank[&i])),
    }
    all
}

#[test]
fn incremental_permutations_match_a_full_sort() {
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut rnd = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let names = ["alpha", "Beta", "beta", "gamma", "Ärger", "zeta", "a", "file10", "file2", "Ω"];
    let mut v = volume();
    let mut next = 100u64;
    let mut dirs = vec![5u64];
    for round in 0..40 {
        for _ in 0..30 {
            match rnd(10) {
                0..=5 => {
                    let dir = rnd(4) == 0;
                    let parent = dirs[rnd(dirs.len() as u64) as usize];
                    let name = format!("{}{}", names[rnd(names.len() as u64) as usize], rnd(5));
                    let r = IndexRecord { id: next, parent, name: name.as_bytes(), size: rnd(50), modified: rnd(20) as i64, attrs: if dir { attr::DIR } else { 0 } };
                    v.upsert(&r);
                    if dir {
                        dirs.push(next);
                    }
                    next += 1;
                }
                6 | 7 if next > 100 => {
                    let id = 100 + rnd(next - 100);
                    if let Some(i) = v.lookup(id) {
                        let dir = v.is_dir(i);
                        let parent = if dir { v.parent_id(i) } else { dirs[rnd(dirs.len() as u64) as usize] };
                        let name = names[rnd(names.len() as u64) as usize].to_uppercase();
                        v.upsert(&IndexRecord { id, parent, name: name.as_bytes(), size: rnd(50), modified: rnd(20) as i64, attrs: if dir { attr::DIR } else { 0 } });
                    }
                }
                _ if next > 100 => v.remove(100 + rnd(next - 100)),
                _ => {}
            }
        }
        v.commit();
        for order in [Order::Name, Order::Size, Order::Modified, Order::Path] {
            v.build_order(order);
            let expected = brute(&v, order);
            let got = &v.perms[order as usize];
            assert_eq!(got, &expected, "round {round} {order:?}");
            for (pos, &i) in got.iter().enumerate() {
                assert_eq!(v.ranks[order as usize][i as usize], pos as u32);
            }
        }
    }
}

#[test]
fn compaction_drops_dead_names_and_bumps_the_epoch() {
    let mut v = volume();
    let long = "x".repeat(200);
    for k in 0..6000u64 {
        v.upsert(&rec(100 + k, 5, &format!("{long}{k}"), false));
    }
    v.commit();
    let before = v.lower.len();
    for k in 0..6000u64 {
        if k % 2 == 0 {
            v.remove(100 + k);
        } else {
            v.upsert(&rec(100 + k, 5, &format!("short{k}"), false));
        }
    }
    v.commit();
    assert_eq!(v.epoch, 1);
    assert!(v.lower.len() < before / 10, "{} vs {before}", v.lower.len());
    assert_eq!(path(&v, 101), r"C:\short1");
    assert_eq!(v.seg_start.len(), v.live() + 1, "live entries plus the root");
}

#[test]
fn names_already_in_lower_case_are_kept_once() {
    let mut v = VolumeIndex::new("C:", b"C:", b'/');
    v.upsert(&IndexRecord { id: 5, parent: 5, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    v.upsert(&IndexRecord { id: 6, parent: 5, name: b"readme.md", size: 0, modified: 0, attrs: 0 });
    v.upsert(&IndexRecord { id: 7, parent: 5, name: "README.md".as_bytes(), size: 0, modified: 0, attrs: 0 });
    v.upsert(&IndexRecord { id: 8, parent: 5, name: "ДОК".as_bytes(), size: 0, modified: 0, attrs: 0 });
    v.commit();
    assert_eq!(v.names, ["README.md".as_bytes(), "ДОК".as_bytes()].concat());
    let named = |v: &VolumeIndex, id| v.name(v.lookup(id).unwrap()).to_vec();
    assert_eq!((named(&v, 6), named(&v, 7), named(&v, 8)), (b"readme.md".to_vec(), b"README.md".to_vec(), "ДОК".as_bytes().to_vec()));
    v.upsert(&IndexRecord { id: 6, parent: 5, name: b"Readme.md", size: 0, modified: 0, attrs: 0 });
    v.commit();
    assert_eq!(named(&v, 6), b"Readme.md");
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn a_single_change_commits_without_touching_every_record() {
    let mut v = volume();
    for d in 0..1000u64 {
        v.upsert(&rec(10 + d, 5, &format!("Folder {d}"), true));
    }
    for i in 0..1_000_000u64 {
        let name = format!("file-{}-{}.txt", i % 7919, i);
        v.upsert(&IndexRecord { id: 100_000 + i, parent: 10 + i % 1000, name: name.as_bytes(), size: i % 5000, modified: (i % 977) as i64, attrs: 0 });
    }
    v.commit();
    for order in [Order::Size, Order::Modified, Order::Path] {
        v.build_order(order);
    }
    let mut times: Vec<f64> = (0..7u64)
        .map(|k| {
            v.upsert(&IndexRecord { id: 10_000_000 + k, parent: 10 + k, name: b"Fresh.txt", size: 1, modified: 1, attrs: 0 });
            let t = std::time::Instant::now();
            v.commit();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    assert!(times[3] < 8.0, "one new file took {:.1} ms to commit into 1M records", times[3]);
}
