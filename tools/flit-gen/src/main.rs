use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

const EXTS: [&str; 10] = ["txt", "jpg", "PNG", "rs", "docx", "pdf", "mp4", "json", "dll", ""];
const WORDS: [&str; 8] = ["report", "Image", "notes", "build", "Invoice", "photo", "data", "Archive"];
const MAX_SIZE: u64 = 4096;
const NESTED_DEPTH: u32 = 8;
const NESTED_BRANCHING: u32 = 2;
const NESTED_FILES_PER_DIR: u32 = 5;

const UNICODE_FILES: [&str; 20] = [
    "Отчёт за 2024 год.docx",
    "привет_мир.txt",
    "Ёжик в тумане.mp4",
    "中文文件.txt",
    "日本語のファイル.pdf",
    "한국어 문서.json",
    "🎉 party.png",
    "👨\u{200D}👩\u{200D}👧\u{200D}👦 family.jpg",
    "🦀.rs",
    "Cafe\u{301}.txt",
    "Caf\u{e9}.txt",
    "Z\u{336}a\u{337}l\u{338}g\u{335}o\u{334}.txt",
    "مرحبا بالعالم.txt",
    "שלום עולם.docx",
    "photo\u{202E}gpj.txt",
    "Ελληνικά.txt",
    "हिन्दी दस्तावेज़.pdf",
    "ภาษาไทย.txt",
    "mixed English русский 中文 🎉.txt",
    "zero\u{200B}width",
];
const UNICODE_DIRS: [&str; 4] = ["Документы", "中文目录", "📁 Emoji", "עברית"];

const VOCAB: &[&str] = &[
    "report", "Invoice", "photo", "backup", "notes", "draft", "budget", "meeting", "Résumé", "café", "naïve", "Straße", "Ångström", "Zürich", "señal", "Łódź",
    "отчёт", "Документ", "фото", "бюджет", "Ёлка", "встреча", "черновик", "Счёт", "архив", "Привет", "έγγραφο", "Αναφορά", "文件", "报告", "照片", "会议记录",
    "ファイル", "写真", "東京", "보고서", "사진", "تقرير", "صورة", "דוח", "תמונה", "दस्तावेज़", "रिपोर्ट", "ภาพถ่าย", "รายงาน", "🎉", "📁", "🦀",
    "👨\u{200D}👩\u{200D}👧", "✅", "🔥", "Ünïcödé", "ﬁnal", "e\u{301}clair", "2024", "v2", "final", "copy", "IMG", "DSC", "scan", "temp",
];
const SEPS: [&str; 4] = [" ", "_", "-", "."];
const TEXT_EXTS: [&str; 8] = ["txt", "md", "json", "csv", "log", "rs", "py", "html"];
const BIN_EXTS: [&str; 10] = ["jpg", "PNG", "mp4", "pdf", "docx", "zip", "exe", "dll", "bin", ""];

fn mixed_name(i: u64) -> (String, bool) {
    let mut x = mix(i ^ 0xA11C_E5ED);
    let words = if x >> 60 == 0 { 8 + (x >> 20) % 5 } else { 1 + x % 4 };
    let mut s = String::new();
    for k in 0..words {
        x = mix(x);
        if k > 0 {
            s.push_str(SEPS[(x % 4) as usize]);
        }
        s.push_str(VOCAB[(x >> 8) as usize % VOCAB.len()]);
    }
    x = mix(x);
    s.push_str(&format!(" {i}"));
    let text = x.is_multiple_of(3);
    let ext = if text { TEXT_EXTS[(x >> 8) as usize % TEXT_EXTS.len()] } else { BIN_EXTS[(x >> 8) as usize % BIN_EXTS.len()] };
    if !ext.is_empty() {
        s.push('.');
        s.push_str(ext);
    }
    (s, text)
}

fn mixed_size(i: u64) -> usize {
    let r = mix(i ^ 0x512E);
    let span = |lo: u64, hi: u64| (lo + (r >> 16) % (hi - lo)) as usize;
    match r % 1000 {
        0..200 => 0,
        200..650 => span(1, 1 << 10),
        650..950 => span(1 << 10, 16 << 10),
        950..995 => span(16 << 10, 64 << 10),
        _ => span(64 << 10, 1 << 20),
    }
}

fn mixed_content(i: u64, size: usize, text: bool, buf: &mut Vec<u8>) {
    buf.clear();
    let mut x = mix(i ^ 0xC0FFEE);
    if text {
        let mut words = 0;
        while buf.len() < size {
            x = mix(x);
            buf.extend_from_slice(VOCAB[(x >> 8) as usize % VOCAB.len()].as_bytes());
            words += 1;
            buf.push(if words % 12 == 0 { b'\n' } else { b' ' });
        }
        let mut end = size;
        while end > 0 && end < buf.len() && buf[end] & 0xC0 == 0x80 {
            end -= 1;
        }
        buf.truncate(end);
        buf.resize(size, b' ');
    } else {
        while buf.len() < size {
            x = mix(x);
            buf.extend_from_slice(&x.to_le_bytes());
        }
        buf.truncate(size);
    }
}

fn write_mixed(dir: &Path, n: u64) -> io::Result<u64> {
    use std::sync::atomic::{AtomicU64, Ordering};
    const BLOCK: u64 = 1024;
    fs::create_dir_all(dir).map_err(|e| at(dir, e))?;
    let next = AtomicU64::new(0);
    let made = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let threads = thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let started = Instant::now();
    thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut buf = Vec::new();
                    loop {
                        let from = next.fetch_add(BLOCK, Ordering::Relaxed);
                        if from >= n {
                            return Ok::<(), io::Error>(());
                        }
                        for i in from..(from + BLOCK).min(n) {
                            let (name, text) = mixed_name(i);
                            let size = mixed_size(i);
                            let p = dir.join(&name);
                            let mut f = match fs::OpenOptions::new().write(true).create_new(true).open(&p) {
                                Ok(f) => f,
                                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                                Err(e) => return Err(at(&p, e)),
                            };
                            mixed_content(i, size, text, &mut buf);
                            io::Write::write_all(&mut f, &buf).map_err(|e| at(&p, e))?;
                            bytes.fetch_add(size as u64, Ordering::Relaxed);
                        }
                        made.fetch_add((from + BLOCK).min(n) - from, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        while !workers.iter().all(|w| w.is_finished()) {
            thread::sleep(std::time::Duration::from_millis(250));
            let done = made.load(Ordering::Relaxed);
            let rate = done as f64 / started.elapsed().as_secs_f64();
            eprint!("\r{done}/{n} files, {:.0} per second, {:.1} GB      ", rate, bytes.load(Ordering::Relaxed) as f64 / 1e9);
        }
        eprintln!();
        workers.into_iter().try_for_each(|w| w.join().expect("writer thread panicked"))
    })?;
    Ok(bytes.into_inner())
}

fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn file_name(seed: u64) -> String {
    let r = mix(seed);
    let word = WORDS[(r >> 8) as usize % WORDS.len()];
    match EXTS[(r % EXTS.len() as u64) as usize] {
        "" => format!("{word}_{seed}"),
        ext => format!("{word}_{seed}.{ext}"),
    }
}

#[derive(Default)]
struct Plan {
    dirs: Vec<PathBuf>,
    files: Vec<(PathBuf, usize)>,
}

impl Plan {
    fn dir(&mut self, rel: PathBuf) {
        self.dirs.push(rel);
    }

    fn file(&mut self, rel: PathBuf, seed: u64) {
        self.files.push((rel, (mix(seed ^ 0x5EED) % MAX_SIZE) as usize));
    }

    fn len(&self) -> usize {
        self.dirs.len() + self.files.len()
    }

    fn write(&self, base: &Path, noise: &[u8]) -> io::Result<u64> {
        fs::create_dir_all(base).map_err(|e| at(base, e))?;
        for d in &self.dirs {
            let p = base.join(d);
            fs::create_dir_all(&p).map_err(|e| at(&p, e))?;
        }
        let threads = thread::available_parallelism().map_or(4, |n| n.get()).min(16);
        let chunk = self.files.len().div_ceil(threads).max(1);
        thread::scope(|s| {
            let workers: Vec<_> = self
                .files
                .chunks(chunk)
                .map(|part| {
                    s.spawn(move || {
                        let mut bytes = 0u64;
                        for (rel, size) in part {
                            let p = base.join(rel);
                            fs::write(&p, &noise[..*size]).map_err(|e| at(&p, e))?;
                            bytes += *size as u64;
                        }
                        Ok::<u64, io::Error>(bytes)
                    })
                })
                .collect();
            workers.into_iter().map(|w| w.join().expect("writer thread panicked")).sum()
        })
    }
}

fn at(path: &Path, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

fn count(dir: &Path) -> io::Result<usize> {
    let mut n = 0;
    for e in fs::read_dir(dir)? {
        let e = e?;
        n += 1;
        if e.file_type()?.is_dir() {
            n += count(&e.path())?;
        }
    }
    Ok(n)
}

fn flat(n: u64) -> Plan {
    let mut p = Plan::default();
    for i in 0..n {
        p.file(file_name(i).into(), i);
    }
    p
}

fn nested_into(p: &mut Plan, rel: &Path, depth: u32, seed: &mut u64) {
    for _ in 0..NESTED_FILES_PER_DIR {
        p.file(rel.join(file_name(*seed)), *seed);
        *seed += 1;
    }
    if depth == NESTED_DEPTH {
        return;
    }
    for k in 0..NESTED_BRANCHING {
        let d = rel.join(format!("level{}_{k}", depth + 1));
        p.dir(d.clone());
        nested_into(p, &d, depth + 1, seed);
    }
}

fn nested() -> Plan {
    let mut p = Plan::default();
    nested_into(&mut p, Path::new(""), 0, &mut 0);
    p
}

fn long_paths() -> Plan {
    let mut p = Plan::default();
    let segment = "deep_directory_segment_".repeat(3);
    let prefix = "very_long_file_name_".repeat(7);
    for branch in 0..3u64 {
        let mut rel = PathBuf::new();
        for level in 0..6u64 {
            rel.push(format!("{branch}{level}_{segment}"));
            p.dir(rel.clone());
            for k in 0..2u64 {
                let seed = branch * 100 + level * 10 + k;
                p.file(rel.join(format!("{prefix}{}", file_name(seed))), seed);
            }
        }
    }
    p
}

fn unicode() -> Plan {
    let mut p = Plan::default();
    for (i, f) in UNICODE_FILES.iter().enumerate() {
        p.file(f.into(), i as u64);
    }
    for d in UNICODE_DIRS {
        p.dir(d.into());
        for (i, f) in UNICODE_FILES.iter().enumerate() {
            p.file(Path::new(d).join(f), i as u64 + 100);
        }
    }
    p
}

#[cfg(windows)]
fn invalid_utf16() -> Option<Plan> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    let wide = |prefix: &str, units: &[u16], suffix: &str| -> PathBuf {
        let mut v: Vec<u16> = prefix.encode_utf16().collect();
        v.extend_from_slice(units);
        v.extend(suffix.encode_utf16());
        OsString::from_wide(&v).into()
    };
    let files = [
        wide("", &[0xD800], "_lone_high_first.txt"),
        wide("lone_low_", &[0xDC00], ".txt"),
        wide("reversed_pair_", &[0xDC00, 0xD800], ".jpg"),
        wide("trailing_high", &[0xDBFF], ""),
        wide("two_highs_", &[0xD83D, 0xD83D], ".json"),
        wide("valid_pair_", &[0xD83D, 0xDE00], ".txt"),
    ];
    let mut p = Plan::default();
    for (i, f) in files.iter().enumerate() {
        p.file(f.clone(), i as u64);
    }
    let dir = wide("dir_", &[0xDFFF], "");
    p.dir(dir.clone());
    for (i, f) in files.iter().enumerate() {
        p.file(dir.join(f), i as u64 + 100);
    }
    Some(p)
}

#[cfg(not(windows))]
fn invalid_utf16() -> Option<Plan> {
    None
}

fn usage() -> ! {
    eprintln!("usage: flit-gen <out_dir> [--flat N]... [--mixed N]... [--nested] [--long] [--unicode] [--invalid-utf16] [--all]");
    std::process::exit(2);
}

fn flat_name(n: u64) -> String {
    if n >= 1000 && n.is_multiple_of(1000) {
        format!("flat_{}k", n / 1000)
    } else {
        format!("flat_{n}")
    }
}

fn run(out: &Path, sets: Vec<(String, Option<Plan>)>) -> io::Result<()> {
    let started = Instant::now();
    fs::create_dir_all(out).map_err(|e| at(out, e))?;
    let root = fs::canonicalize(out).map_err(|e| at(out, e))?;
    let noise: Vec<u8> = (0..MAX_SIZE).map(|i| mix(i) as u8).collect();
    println!("output: {}", root.display());
    println!("{:<16} {:>8} {:>12} {:>9}  status", "set", "entries", "bytes", "time");
    for (name, plan) in sets {
        let t = Instant::now();
        let Some(plan) = plan else {
            println!("{name:<16} {:>8} {:>12} {:>9}  skipped: lone surrogates need Windows (UTF-16 names)", "-", "-", "-");
            continue;
        };
        let dir = root.join(&name);
        if dir.is_dir() && count(&dir).ok() == Some(plan.len()) {
            println!("{name:<16} {:>8} {:>12} {:>8.2}s  already complete", plan.len(), "-", t.elapsed().as_secs_f64());
            continue;
        }
        let bytes = plan.write(&dir, &noise)?;
        let found = count(&dir).map_err(|e| at(&dir, e))?;
        let status = if found == plan.len() { "created".to_string() } else { format!("created, but folder holds {found} entries") };
        println!("{name:<16} {:>8} {bytes:>12} {:>8.2}s  {status}", plan.len(), t.elapsed().as_secs_f64());
    }
    println!("total {:.2}s", started.elapsed().as_secs_f64());
    Ok(())
}

fn main() {
    let mut out: Option<PathBuf> = None;
    let mut flats: Vec<u64> = Vec::new();
    let mut mixed: Vec<u64> = Vec::new();
    let (mut want_nested, mut want_long, mut want_unicode, mut want_invalid) = (false, false, false, false);
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--flat") => flats.push(args.next().and_then(|n| n.to_str()?.parse().ok()).unwrap_or_else(|| usage())),
            Some("--mixed") => mixed.push(args.next().and_then(|n| n.to_str()?.parse().ok()).unwrap_or_else(|| usage())),
            Some("--nested") => want_nested = true,
            Some("--long") => want_long = true,
            Some("--unicode") => want_unicode = true,
            Some("--invalid-utf16") => want_invalid = true,
            Some("--all") => {
                flats.extend([10_000, 100_000]);
                (want_nested, want_long, want_unicode, want_invalid) = (true, true, true, true);
            }
            Some(s) if s.starts_with("--") => usage(),
            _ if out.is_none() => out = Some(a.into()),
            _ => usage(),
        }
    }
    let Some(out) = out else { usage() };
    flats.sort_unstable();
    flats.dedup();
    let mut sets: Vec<(String, Option<Plan>)> = flats.into_iter().map(|n| (flat_name(n), Some(flat(n)))).collect();
    if want_nested {
        sets.push(("nested".into(), Some(nested())));
    }
    if want_long {
        sets.push(("long_paths".into(), Some(long_paths())));
    }
    if want_unicode {
        sets.push(("unicode".into(), Some(unicode())));
    }
    if want_invalid {
        sets.push(("invalid_utf16".into(), invalid_utf16()));
    }
    if sets.is_empty() && mixed.is_empty() {
        usage();
    }
    let result = if sets.is_empty() { Ok(()) } else { run(&out, sets) }.and_then(|()| mixed.iter().try_for_each(|&n| run_mixed(&out, n)));
    if let Err(e) = result {
        eprintln!("flit-gen: {e}");
        std::process::exit(1);
    }
}

fn mixed_dir_name(n: u64) -> String {
    match n {
        n if n >= 1_000_000 && n.is_multiple_of(1_000_000) => format!("mixed_{}m", n / 1_000_000),
        n if n >= 1000 && n.is_multiple_of(1000) => format!("mixed_{}k", n / 1000),
        n => format!("mixed_{n}"),
    }
}

fn run_mixed(out: &Path, n: u64) -> io::Result<()> {
    let t = Instant::now();
    let dir = out.join(mixed_dir_name(n));
    let name = mixed_dir_name(n);
    if dir.is_dir() && count(&dir).ok() == Some(n as usize) {
        println!("{name:<16} {n:>8} {:>12} {:>8.2}s  already complete", "-", t.elapsed().as_secs_f64());
        return Ok(());
    }
    let bytes = write_mixed(&dir, n)?;
    let found = count(&dir).map_err(|e| at(&dir, e))?;
    let status = if found == n as usize { "created".to_string() } else { format!("created, but folder holds {found} entries") };
    println!("{name:<16} {n:>8} {bytes:>12} {:>8.2}s  {status}", t.elapsed().as_secs_f64());
    Ok(())
}
