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
