use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use flit_core::listing::{Listing, Meta};
use flit_core::sort::{self, CaseMode, SortField, SortKeys, SortSpec, ViewSpec};
use std::hint::black_box;

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
    let n100k = names(100_000);
    let (l, k) = listing(&n100k);

    c.bench_function("listing_build_100k (utf16 → wtf8 + keys)", |b| b.iter(|| black_box(listing(&n100k))));

    let view = ViewSpec::default();
    c.bench_function("sort_name_100k", |b| {
        b.iter_batched(Vec::new, |mut o| sort::sort(&l, &k, view, &mut o), BatchSize::LargeInput)
    });

    let by_size = ViewSpec { sort: SortSpec { field: SortField::Size, descending: true }, show_hidden: false };
    c.bench_function("sort_size_100k", |b| {
        b.iter_batched(Vec::new, |mut o| sort::sort(&l, &k, by_size, &mut o), BatchSize::LargeInput)
    });

    let by_type = ViewSpec { sort: SortSpec { field: SortField::Type, descending: false }, show_hidden: false };
    c.bench_function("sort_type_100k", |b| {
        b.iter_batched(Vec::new, |mut o| sort::sort(&l, &k, by_type, &mut o), BatchSize::LargeInput)
    });

    let (a, z) = ("file0123 copy (2).txt".as_bytes(), "file123 copy (10).txt".as_bytes());
    c.bench_function("natural_cmp_pair", |b| b.iter(|| sort::natural_cmp(black_box(a), black_box(z), CaseMode::Insensitive)));
}

criterion_group!(core, benches);
criterion_main!(core);
