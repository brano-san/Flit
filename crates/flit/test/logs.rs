use super::*;
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let until = Instant::now() + Duration::from_secs(5);
        while std::fs::remove_dir_all(&self.0).is_err() && self.0.exists() && Instant::now() < until {
            std::thread::yield_now();
        }
    }
}

fn temp(tag: &str) -> Temp {
    let d = std::env::temp_dir().join(format!("flit-logs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Temp(d)
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap_or_default()
}

#[test]
fn lines_reach_the_file_in_order_without_a_shutdown() {
    let d = temp("order");
    let sink = Sink::start(Some(d.0.clone()), false).unwrap();
    for k in 0..100 {
        sink.line(format!("line {k}\n").into_bytes());
    }
    assert!(sink.flush(Duration::from_secs(5)));
    let text = read(&d.0, FILE);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 100);
    assert!(lines.iter().enumerate().all(|(k, l)| *l == format!("line {k}")), "a flush leaves every line on disk while the writer is still running");
}

#[test]
fn the_file_rotates_at_the_size_cap_and_keeps_one_older_file() {
    let d = temp("rotate");
    let sink = Sink::start(Some(d.0.clone()), false).unwrap();
    let line = format!("{}\n", "x".repeat(1023));
    for _ in 0..(5 * MAX_BYTES as usize / 2 / line.len()) {
        sink.line(line.clone().into_bytes());
    }
    assert!(sink.flush(Duration::from_secs(10)));
    let (now, old) = (std::fs::metadata(d.0.join(FILE)).map_or(0, |m| m.len()), std::fs::metadata(d.0.join(OLD)).unwrap().len());
    assert!(now < MAX_BYTES && (MAX_BYTES..MAX_BYTES + line.len() as u64).contains(&old), "{now} {old}");
    assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), 2);
}

#[test]
fn a_restart_appends_and_an_oversized_file_rotates_first() {
    let d = temp("restart");
    std::fs::create_dir_all(&d.0).unwrap();
    std::fs::write(d.0.join(FILE), "previous session\n").unwrap();
    let sink = Sink::start(Some(d.0.clone()), false).unwrap();
    sink.line(b"next session\n".to_vec());
    assert!(sink.flush(Duration::from_secs(5)));
    assert_eq!(read(&d.0, FILE), "previous session\nnext session\n");
    drop(sink);
    std::fs::write(d.0.join(FILE), vec![b'a'; MAX_BYTES as usize]).unwrap();
    let sink = Sink::start(Some(d.0.clone()), false).unwrap();
    sink.line(b"fresh\n".to_vec());
    assert!(sink.flush(Duration::from_secs(5)));
    assert_eq!(read(&d.0, FILE), "fresh\n");
    assert_eq!(std::fs::metadata(d.0.join(OLD)).unwrap().len(), MAX_BYTES);
}

#[test]
fn a_panic_record_names_the_thread_the_place_and_the_backtrace() {
    let record = Arc::new(Mutex::new(String::new()));
    let seen = record.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("flit-test-panic") {
            *seen.lock().unwrap() = panic_record(info);
        }
    }));
    let worker = std::thread::Builder::new().name("flit-test-panic".into()).spawn(|| panic!("the test panics on purpose")).unwrap();
    assert!(worker.join().is_err());
    std::panic::set_hook(previous);
    let text = record.lock().unwrap().clone();
    assert!(text.contains(" PANIC thread 'flit-test-panic' panicked at ") && text.contains("logs.rs") && text.contains("the test panics on purpose"), "{text}");
    assert!(text.lines().count() > 2, "a backtrace follows the message: {text}");
    assert!(text.starts_with(char::is_numeric), "the record starts with a timestamp: {text}");
}

#[test]
fn levels_follow_the_settings_choices() {
    assert_eq!(LEVELS.len(), 5);
    assert!(LEVELS.windows(2).all(|w| w[0] < w[1]));
    let doc = flit_core::ui::settings::parse("{\"advanced.log_level\": \"warn\"}");
    let k = flit_core::ui::settings::index(flit_core::ui::settings::LOG_LEVEL).unwrap();
    assert_eq!(doc.values, vec![(k, flit_core::ui::Value::Choice(2))]);
    assert_eq!(LEVELS[2], LevelFilter::WARN);
    assert_eq!(LEVELS[flit_core::ui::settings::LOG_DEFAULT], LevelFilter::ERROR);
}

#[test]
#[ignore]
fn bench_the_cost_of_a_log_line() {
    let d = temp("bench");
    let sink = Sink::start(Some(d.0.clone()), false).unwrap();
    let (filter, _handle) = reload::Layer::new(LevelFilter::INFO);
    let subscriber = tracing_subscriber::registry().with(filter).with(tracing_subscriber::fmt::layer().with_thread_names(true).with_writer(sink.clone()));
    let n = 100_000u32;
    tracing::subscriber::with_default(subscriber, || {
        for k in 0..1000 {
            tracing::info!("warm-up {k}");
        }
        sink.flush(Duration::from_secs(10));
        let t = Instant::now();
        for k in 0..n {
            tracing::debug!("filtered out {k}");
        }
        let filtered = t.elapsed();
        let t = Instant::now();
        for k in 0..n {
            tracing::warn!("operation failed: Access denied (0x80070005) {k}");
        }
        let written = t.elapsed();
        let t = Instant::now();
        sink.flush(Duration::from_secs(60));
        let drained = t.elapsed();
        println!("filtered debug!: {:.1} ns per call", filtered.as_nanos() as f64 / f64::from(n));
        println!("written warn!: {:.2} us per call on the calling thread", written.as_nanos() as f64 / f64::from(n) / 1e3);
        println!("writer thread drained the rest in {:.1} ms", drained.as_secs_f64() * 1e3);
    });
    println!("file size {} bytes", std::fs::metadata(d.0.join(FILE)).map_or(0, |m| m.len()) + std::fs::metadata(d.0.join(OLD)).map_or(0, |m| m.len()));
}

#[test]
fn the_crash_report_is_the_newest_dump_since_the_marker() {
    let d = Temp(std::env::temp_dir().join(format!("flit-crash-report-{}", std::process::id())));
    std::fs::create_dir_all(&d.0).unwrap();
    let now = std::time::SystemTime::now();
    let put = |name: &str, age: u64| {
        let p = d.0.join(name);
        std::fs::write(&p, b"MDMP").unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(now - Duration::from_secs(age)).unwrap();
    };
    put("flit-crash-old.dmp", 600);
    put("flit-crash-a.dmp", 20);
    put("flit-crash-b.dmp", 10);
    put("flit-crash-b.txt", 5);
    assert_eq!(crash_report(&d.0, now - Duration::from_secs(60)), Some(d.0.join("flit-crash-b.dmp")));
    assert_eq!(crash_report(&d.0, now), None);
}
