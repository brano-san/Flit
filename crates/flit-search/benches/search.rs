use criterion::{criterion_group, criterion_main, Criterion};
use flit_core::attr;
use flit_search::engine::Engine;
use flit_search::index::{IndexRecord, IndexSink, VolumeIndex};
use flit_search::proto::{Sort, SortKey};
use flit_search::query::{parse, Clock};
use std::hint::black_box;
use std::time::{Duration, Instant};

const NOW: i64 = 133_700_000_000_000_000;

pub fn synthetic(n: usize) -> VolumeIndex {
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let exts = ["txt", "jpg", "png", "rs", "docx", "pdf", "mp4", "json", "dll", "exe", "h", "cpp", ""];
    let dir_names = ["src", "bin", "obj", "node_modules", "Documents", "Pictures", "build", "target", "packages", "Windows", "System32", "Program Files"];
    let mut v = VolumeIndex::new("C:", b"C:\\", b'\\');
    v.upsert(&IndexRecord { id: 5, parent: 5, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    let dirs = (n / 10).max(1);
    let mut name = String::new();
    for i in 0..n {
        let r = rnd();
        let id = 100 + i as u64;
        let is_dir = i < dirs;
        let parent = if i == 0 { 5 } else if is_dir { 100 + (r % i as u64).max(i as u64 / 2) } else { 100 + r % dirs as u64 };
        name.clear();
        if is_dir {
            name.push_str(dir_names[(r >> 8) as usize % dir_names.len()]);
            name.push_str(&format!(" {}", r % 997));
        } else {
            let ext = exts[(r >> 20) as usize % exts.len()];
            match r >> 60 {
                0..=4 => name.push_str(&format!("file{}", r % 100_000)),
                5..=8 => name.push_str(&format!("IMG_{:04}", r % 10_000)),
                9..=11 => name.push_str(&format!("Документ {} (копия)", r % 5000)),
                _ => name.push_str(&format!("Report-{}-v{}_Final", r % 3000, i % 17)),
            }
            if !ext.is_empty() {
                name.push('.');
                name.push_str(ext);
            }
        }
        let size = if is_dir { 0 } else { (r >> 16) % (64 << 20) };
        let modified = NOW - ((r >> 24) % (3 * 365 * 86_400)) as i64 * 10_000_000;
        v.upsert(&IndexRecord { id, parent, name: name.as_bytes(), size, modified, attrs: if is_dir { attr::DIR } else { 0 } });
    }
    v.commit();
    v
}

fn bench_size(c: &mut Criterion, n: usize, label: &str) {
    let started = Instant::now();
    let v = synthetic(n);
    eprintln!("{label}: built {} entries in {:?}", v.live(), started.elapsed());
    let engine = Engine { volumes: vec![v], version: 1 };
    let clock = Clock { now: NOW, utc_offset_min: 0 };
    let never = || false;
    let mut g = c.benchmark_group(label);
    g.sample_size(20).measurement_time(Duration::from_secs(4));
    let queries = [
        "r", "re", "rep", "repo", "repor", "report", "report-12", "докум", "img_00", "*.jpg", "file1*.txt", "ext:rs;h;cpp", "size:>32mb", "dm:2023", "report ext:pdf size:<1mb",
        "path:src", "regex:^img_\\d{3}5", "",
    ];
    for key in [SortKey::Name, SortKey::Size] {
        for q in queries {
            let query = parse(q, clock).unwrap();
            let sort = Sort { key, descending: false };
            g.bench_function(format!("{key:?}/{q}"), |b| {
                b.iter(|| {
                    let mut rs = engine.search(black_box(&query), sort, &never).unwrap();
                    black_box(engine.rows(&mut rs, 0, 64));
                })
            });
        }
    }
    g.finish();
}

fn benches(c: &mut Criterion) {
    bench_size(c, 1_000_000, "search_1m");
    if std::env::var_os("FLIT_BENCH_10M").is_some() {
        bench_size(c, 10_000_000, "search_10m");
    }
}

criterion_group!(search, benches);
criterion_main!(search);
