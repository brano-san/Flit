use super::*;
use std::time::Duration;

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> Temp {
    let d = std::env::temp_dir().join(format!("flit-settings-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    Temp(d)
}

fn worker(path: &Path, first: &str) -> (Sender<Job>, Receiver<Reply>, JoinHandle<()>) {
    let (jobs, rx) = unbounded();
    let (tx, replies) = unbounded();
    let (path, first) = (path.to_path_buf(), first.to_owned());
    let thread = std::thread::spawn(move || work(Some(path), None, first, rx, tx, Arc::new(|| {})));
    (jobs, replies, thread)
}

fn reply(r: &Receiver<Reply>) -> Reply {
    r.recv_timeout(Duration::from_secs(5)).expect("a reply")
}

#[test]
fn writes_are_atomic_coalesced_and_leave_no_temporary_files() {
    let d = temp("write");
    let file = d.0.join("Flit").join("settings.json");
    let (jobs, replies, thread) = worker(&file, "");
    for k in 0..50 {
        jobs.send(Job::Write(format!("{{\"n\": {k}}}"))).unwrap();
    }
    jobs.send(Job::Check).unwrap();
    drop(jobs);
    thread.join().unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"n\": 49}");
    assert!(replies.try_recv().is_err(), "our own write is not reported as a change");
    let names: Vec<_> = std::fs::read_dir(file.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, ["settings.json"]);
}

#[test]
fn edits_from_outside_are_reported_once_and_broken_files_are_backed_up() {
    let d = temp("check");
    let file = d.0.join("settings.json");
    write_atomic(&file, b"{}").unwrap();
    std::fs::write(&file, "{\"files.show_hidden\": true}").unwrap();
    let (jobs, replies, thread) = worker(&file, "{}");
    jobs.send(Job::Check).unwrap();
    jobs.send(Job::Check).unwrap();
    assert!(matches!(reply(&replies), Reply::Changed(t) if t.contains("show_hidden")));
    std::fs::write(&file, "{ broken").unwrap();
    jobs.send(Job::Backup).unwrap();
    jobs.send(Job::Write("{\"fresh\": 1}".into())).unwrap();
    let export = d.0.join("out").join("flit-settings.json");
    jobs.send(Job::Export(export.clone(), "{\"tags\": []}".into())).unwrap();
    jobs.send(Job::Import(export.clone())).unwrap();
    jobs.send(Job::Import(d.0.join("missing.json"))).unwrap();
    assert!(matches!(reply(&replies), Reply::Exported(Ok(p)) if p == export));
    assert!(matches!(reply(&replies), Reply::Imported(Ok(t)) if t == "{\"tags\": []}"));
    assert!(matches!(reply(&replies), Reply::Imported(Err(_))));
    drop(jobs);
    thread.join().unwrap();
    assert!(replies.try_recv().is_err(), "the second check saw nothing new");
    assert_eq!(std::fs::read_to_string(d.0.join("settings.broken.json")).unwrap(), "{ broken");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"fresh\": 1}");
}

#[test]
fn the_file_is_written_only_when_it_is_missing_broken_incomplete_or_wrong() {
    let d = temp("adopt");
    let file = d.0.join("settings.json");
    let defaults = settings::defaults().to_vec();
    let full = settings::render(&defaults, &Map::new(), None);
    for (text, written) in [(None, true), (Some("{ nope".to_owned()), true), (Some("{\"files.show_hidden\": true}".to_owned()), true), (Some(full.replace("\"files.size_units\": \"binary\"", "\"files.size_units\": \"metric\"")), true), (Some(full.clone()), false)] {
        let _ = std::fs::remove_dir_all(&d.0);
        if let Some(t) = &text {
            write_atomic(&file, t.as_bytes()).unwrap();
        }
        let doc = text.as_deref().map(settings::parse).unwrap_or_default();
        let mut s = Settings::start::<crate::P>(Some(file.clone()), text.clone(), None, Arc::new(|| {}));
        s.adopt(&doc, defaults.clone());
        s.finish();
        let now = std::fs::read_to_string(&file).ok();
        assert_eq!(now.as_deref() == Some(full.as_str()) && now != text, written, "{text:?}");
        assert_eq!(d.0.join("settings.broken.json").exists(), doc.broken);
    }
}

#[test]
fn unknown_keys_survive_a_rewrite_and_cache_limits_are_read() {
    let doc = settings::parse("{\"advanced.thumb_cache_mb\": 1200, \"theme.name\": \"Storm\"}");
    assert_eq!(thumb_cache_mb(&doc), Some(1200));
    let mut s = Settings::start::<crate::P>(None, None, None, Arc::new(|| {}));
    s.adopt(&doc, settings::defaults().to_vec());
    assert!(s.text().contains("\"theme.name\": \"Storm\""));
    assert_eq!(cache_limit(&s.values), Some(500));
    s.adopt(&settings::parse("{ broken"), settings::defaults().to_vec());
    assert!(s.text().contains("\"theme.name\": \"Storm\""), "a broken file does not drop the unknown keys read before");
    s.finish();
    assert_eq!(thumb_cache_mb(&settings::parse("{\"advanced.thumb_cache_mb\": 1}")), None, "out of range values do not reach the cache");
}
