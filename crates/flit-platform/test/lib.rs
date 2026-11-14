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

#[test]
fn errors_name_the_cause_once_in_the_style_of_the_system() {
    let reason = flit_core::fmt::io_reason(&std::io::Error::from_raw_os_error(13));
    let e = PlatformError::new(13, format!("/root/x: {reason}"));
    let shown = e.to_string();
    if cfg!(windows) {
        assert!(shown.ends_with("(0x0000000D)"), "{shown}");
    } else {
        assert_eq!(shown, format!("/root/x: {reason}"));
        assert_eq!(PlatformError::new(-1, "Deletion was cancelled").to_string(), "Deletion was cancelled");
    }
}

fn shown(patterns: &(String, String)) -> String {
    let secs = 1_710_000_000 + 15 * 3600 + 7 * 60 + 9 - 1_710_000_000 % 86_400;
    let mut out = String::new();
    flit_core::fmt::picture(&mut out, &patterns.0, secs);
    out.push(' ');
    flit_core::fmt::picture(&mut out, &patterns.1, secs);
    out
}

#[test]
fn dates_follow_the_locale_of_the_linux_desktop_without_seconds() {
    let russian = posix_date_patterns("%d.%m.%Y", "%T").unwrap();
    assert_eq!(russian, ("dd.MM.yyyy".to_owned(), "HH:mm".to_owned()));
    assert_eq!(shown(&russian), "09.03.2024 15:07");
    let american = posix_date_patterns("%m/%d/%Y", "%r").unwrap();
    assert_eq!(shown(&american), format!("03/09/2024 03:07 {}", flit_core::i18n::S::FormatDatePm.t()));
    assert_eq!(shown(&posix_date_patterns("%Y-%m-%d", "%H:%M:%S").unwrap()), "2024-03-09 15:07");
    assert_eq!(shown(&posix_date_patterns("%-d/%-m/%y", "%k.%M.%S").unwrap()), "9/3/24 15.07");
    assert_eq!(shown(&posix_date_patterns("%Y年%m月%d日", "%H時%M分").unwrap()), "2024年03月09日 15時07分");
}

#[test]
fn letters_between_locale_fields_stay_text_and_odd_formats_keep_the_default() {
    let spanish = posix_date_patterns("%d de %b de %Y", "%Hh%M").unwrap();
    assert_eq!(spanish.0, "dd' de 'MMM' de 'yyyy");
    assert_eq!(shown(&spanish), format!("09 de {} de 2024 15h07", flit_core::i18n::S::FormatMonthShortMar.t()));
    assert_eq!(shown(&posix_date_patterns("%d'%m'%Y", "%R").unwrap()), "09'03'2024 15:07");
    assert_eq!(posix_date_patterns("%x", "%X"), None, "an unknown conversion falls back to the default");
    assert_eq!(posix_date_patterns("%d.%m.%Y", "%H:%"), None, "a cut-off format falls back");
    assert_eq!(posix_date_patterns("%Y", "%H:%M"), None, "a date without day and month falls back");
    assert_eq!(posix_date_patterns("%d.%m.%Y", "%p"), None, "a time without hours and minutes falls back");
}

fn asking_room(answers: Vec<Option<Resolve>>) -> (Job, Arc<std::sync::Mutex<Vec<Conflict>>>) {
    let ctl = OpCtl::new(Some(Resolve::Skip));
    let left = std::sync::Mutex::new(answers);
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (answer, s) = (ctl.clone(), seen.clone());
    let job = Job::asking(
        ctl,
        Box::new(|_| {}),
        Box::new(move |c| {
            s.lock().unwrap().push(c);
            match left.lock().unwrap().remove(0) {
                Some(choice) => answer.answer(choice, false),
                None => answer.cancel(),
            }
        }),
        Box::new(|_| {}),
    );
    (job, seen)
}

#[test]
fn short_of_space_asks_even_with_a_conflict_policy_and_retry_measures_again() {
    let dest = FsPath::parse(r"E:\backup").unwrap();
    let free = std::sync::Mutex::new(vec![3, 5, 50]);
    let measure = || Some(free.lock().unwrap().remove(0));
    let (job, seen) = asking_room(vec![Some(flit_core::limits::ROOM_RETRY), Some(flit_core::limits::ROOM_RETRY)]);
    assert_eq!(job.room(&dest, 10, measure), Ok(()), "enough space after the second retry");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.iter().map(|c| c.to.size).collect::<Vec<_>>(), [3, 5], "each retry shows the free space measured again");
    assert!(seen.iter().all(flit_core::limits::is_short_of_space));
    assert_eq!(job.ctl().policy(), Some(Resolve::Skip), "the answers never become the conflict policy");

    let (job, seen) = asking_room(vec![Some(flit_core::limits::ROOM_ANYWAY)]);
    assert_eq!(job.room(&dest, 10, || Some(3)), Ok(()));
    assert_eq!(seen.lock().unwrap().len(), 1);
    let (job, _) = asking_room(vec![None]);
    assert_eq!((job.room(&dest, 10, || Some(3)), job.cancelled()), (Err(None), true));
}

#[test]
fn short_of_space_without_anyone_to_ask_refuses_and_enough_space_asks_nothing() {
    let dest = FsPath::parse(r"E:\backup").unwrap();
    assert_eq!(Job::detached(Resolve::Replace).room(&dest, 10, || Some(3)), Err(Some(3)), "a policy never copies past the free space");
    let (job, seen) = asking_room(Vec::new());
    assert_eq!(job.room(&dest, 10, || Some(10)), Ok(()));
    assert_eq!(job.room(&dest, 10, || None), Ok(()), "an unknown free space does not stop the copy");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn progress_from_a_paused_job_holds_the_worker_until_it_resumes() {
    let job = Job::detached(Resolve::Skip);
    job.ctl().pause(true);
    let worker = {
        let job = job.clone();
        std::thread::spawn(move || job.update(|p| p.items += 1))
    };
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(job.progress().items, 0);
    job.ctl().pause(false);
    worker.join().unwrap();
    assert_eq!(job.progress().items, 1);
}
