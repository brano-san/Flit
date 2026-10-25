use super::*;

#[test]
fn queries_parse_into_conditions() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        for q in ["kind:music", "author:\"Ann\" size:>1MB", "report datemodified:today"] {
            assert!(condition(q).is_ok(), "{q}");
        }
    }
}

#[test]
#[ignore = "needs the Windows Search service; reads only file metadata under C:\\Windows"]
fn bench_windows_search() {
    let folder = FsPath::parse(r"C:\Windows\System32\drivers").unwrap();
    let query = std::env::var("FLIT_AQS").unwrap_or_else(|_| "ext:.sys".into());
    for pass in 0..3 {
        let started = std::time::Instant::now();
        let mut first = None;
        let mut hits = Vec::new();
        search(Some(&folder), &query, &|| true, &mut |p, m| {
            first.get_or_insert(started.elapsed());
            hits.push((p, m));
        })
        .unwrap();
        let sized = hits.iter().filter(|(_, m)| m.size > 0 && m.modified > 0).count();
        println!("BENCH Windows Search pass {pass}: {} results ({sized} with size and date), first after {:.1} ms, all after {:.1} ms", hits.len(), first.map_or(f64::NAN, |d| d.as_secs_f64() * 1e3), started.elapsed().as_secs_f64() * 1e3);
    }
}
