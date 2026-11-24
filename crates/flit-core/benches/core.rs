use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use flit_core::listing::{DirSnapshot, Listing, Meta};
use flit_core::sort::{self, CaseMode, SortField, SortKeys, SortSpec, ViewSpec};
use flit_core::view::DirView;
use flit_core::FsPath;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn names(n: usize) -> Vec<Vec<u16>> {
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let exts = ["txt", "jpg", "PNG", "rs", "docx", "pdf", "mp4", "json", "dll", ""];
    (0..n)
        .map(|i| {
            let r = rnd();
            let ext = exts[(r % exts.len() as u64) as usize];
            let stem = match r >> 60 {
                0..=4 => format!("file{}", r % 100_000),
                5..=8 => format!("IMG_{:04}", r % 10_000),
                9..=11 => format!("Документ {} (копия)", r % 5000),
                _ => format!("Report-{}-v{}_Final", r % 3000, i % 17),
            };
            let s = if ext.is_empty() { stem } else { format!("{stem}.{ext}") };
            s.encode_utf16().collect()
        })
        .collect()
}

fn listing(names: &[Vec<u16>]) -> (Listing, SortKeys) {
    let mut l = Listing::default();
    let mut k = SortKeys::new(CaseMode::Insensitive);
    for (i, n) in names.iter().enumerate() {
        let meta = Meta { size: (i as u64 * 7919) % 1_000_000, modified: i as i64 * 31, ..Default::default() };
        let idx = l.push_utf16(n, meta);
        k.push(l.name(idx));
    }
    (l, k)
}

fn benches(c: &mut Criterion) {
    let Some(path) = FsPath::parse(r"C:\bench") else {
        eprintln!("invalid synthetic directory path");
        std::process::exit(1);
    };
    let n100k = names(100_000);
    let (l, k) = listing(&n100k);

    c.bench_function("listing_build_100k (utf16 → wtf8 + keys)", |b| b.iter(|| black_box(listing(&n100k))));

    let view = ViewSpec::default();
    c.bench_function("sort_name_100k", |b| b.iter_batched(Vec::new, |mut o| sort::sort(&l, &k, view, &mut o), BatchSize::LargeInput));

    let by_size = ViewSpec { sort: SortSpec { field: SortField::Size, descending: true, ..Default::default() }, show_hidden: false, ..Default::default() };
    c.bench_function("sort_size_100k", |b| b.iter_batched(Vec::new, |mut o| sort::sort(&l, &k, by_size, &mut o), BatchSize::LargeInput));

    let by_type = ViewSpec { sort: SortSpec { field: SortField::Type, descending: false, ..Default::default() }, show_hidden: false, ..Default::default() };
    c.bench_function("sort_type_100k", |b| b.iter_batched(Vec::new, |mut o| sort::sort(&l, &k, by_type, &mut o), BatchSize::LargeInput));

    let (a, z) = ("file0123 copy (2).txt".as_bytes(), "file123 copy (10).txt".as_bytes());
    c.bench_function("natural_cmp_pair", |b| b.iter(|| sort::natural_cmp(black_box(a), black_box(z), CaseMode::Insensitive)));

    let mut grouped = Listing::default();
    let mut grouped_keys = SortKeys::new(CaseMode::Insensitive);
    for i in 0..100_000 {
        let n = format!("file{i}.ext{}", i % 4000);
        grouped.push(n.as_bytes(), Meta::default());
        grouped_keys.push(n.as_bytes());
    }
    let grouped_view = ViewSpec { group: sort::GroupSpec { field: sort::GroupField::Type, ..Default::default() }, ..Default::default() };
    let mut order = Vec::new();
    sort::sort(&grouped, &grouped_keys, grouped_view, &mut order);
    let mut grouped = DirView::new(Arc::new(DirSnapshot { gen: 1, path: path.clone(), listing: grouped, keys: grouped_keys, order, view: grouped_view, done: true, error: None, elapsed: Duration::ZERO }));
    c.bench_function("fold_4000_groups_100k", |b| {
        b.iter_custom(|iterations| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iterations {
                grouped.fold_all(false);
                let start = Instant::now();
                grouped.fold_all(true);
                elapsed += start.elapsed();
            }
            elapsed
        });
    });

    filter_benches(c, path, l, k);
}

fn filter_benches(c: &mut Criterion, path: FsPath, listing: Listing, keys: SortKeys) {
    let view = ViewSpec::default();
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = DirSnapshot { gen: 1, path, listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO };
    let mut v = DirView::new(Arc::new(snap));
    let order = v.snap().order.clone();
    c.bench_function("merge_sort_100k_unchanged", |b| {
        b.iter_batched(|| order.clone(), |order| v.merge_order(order, view, 100_000), BatchSize::LargeInput);
    });
    c.bench_function("filter_100k_ascii (keystroke from no filter)", |b| {
        b.iter(|| {
            v.set_filter("");
            v.set_filter(black_box("report"));
        })
    });
    c.bench_function("filter_100k_cyrillic", |b| {
        b.iter(|| {
            v.set_filter("");
            v.set_filter(black_box("копия"));
        })
    });
    c.bench_function("filter_100k_ext", |b| {
        b.iter(|| {
            v.set_filter("");
            v.set_filter(black_box("ext:jpg;png"));
        })
    });
    c.bench_function("filter_100k_wildcard", |b| {
        b.iter(|| {
            v.set_filter("");
            v.set_filter(black_box("img_*.png"));
        })
    });
    c.bench_function("filter_100k_narrowing (fi → fil)", |b| {
        b.iter(|| {
            v.set_filter("fi");
            v.set_filter(black_box("fil"));
        })
    });
}

criterion_group!(core, benches);
criterion_main!(core);
