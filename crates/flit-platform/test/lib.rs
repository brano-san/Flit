use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Held(Arc<AtomicUsize>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[test]
fn the_system_stays_awake_from_the_first_running_job_until_the_last_one_ends() {
    let held = Arc::new(AtomicUsize::new(0));
    let asked = AtomicUsize::new(0);
    let acquire = || {
        asked.fetch_add(1, Ordering::AcqRel);
        held.fetch_add(1, Ordering::AcqRel);
        Some(Held(held.clone()))
    };
    let mut awake = KeepAwake::default();
    awake.track(false, acquire);
    assert_eq!((asked.load(Ordering::Acquire), awake.held()), (0, false), "nothing runs, nothing is held");
    awake.track(true, acquire);
    awake.track(true, acquire);
    assert_eq!((asked.load(Ordering::Acquire), held.load(Ordering::Acquire), awake.held()), (1, 1, true), "one hold while jobs keep running");
    awake.track(false, acquire);
    assert_eq!((held.load(Ordering::Acquire), awake.held()), (0, false), "the hold ends with the last job");
    awake.track(true, acquire);
    assert_eq!((asked.load(Ordering::Acquire), held.load(Ordering::Acquire)), (2, 1));
    drop(awake);
    assert_eq!(held.load(Ordering::Acquire), 0);

    let mut refused = KeepAwake::<Held>::default();
    let tries = AtomicUsize::new(0);
    for _ in 0..3 {
        refused.track(true, || {
            tries.fetch_add(1, Ordering::AcqRel);
            None
        });
    }
    assert_eq!((tries.load(Ordering::Acquire), refused.held()), (1, false), "a refused hold is not retried on every frame");
}

#[test]
fn only_jobs_with_someone_to_ask_report_failures() {
    let f = Failure { path: FsPath::parse(r"C:\a").unwrap(), error: "Access is denied".into(), fatal: false };
    let quiet = Job::detached(Resolve::Skip);
    assert!(!quiet.asks_failures());
    assert_eq!(quiet.fail(f.clone()), Some(Recover::Skip));
    let ctl = OpCtl::new(None);
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (answer, s) = (ctl.clone(), seen.clone());
    let job = Job::asking(
        ctl,
        Box::new(|_| {}),
        Box::new(|_| {}),
        Box::new(move |f| {
            s.lock().unwrap().push(f);
            answer.answer_failure(Recover::Retry, false);
        }),
    );
    assert!(job.asks_failures());
    assert_eq!(job.fail(f.clone()), Some(Recover::Retry));
    assert_eq!(*seen.lock().unwrap(), [f]);
}
