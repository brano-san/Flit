use super::*;

const DEADLINE: Duration = Duration::from_secs(3);

#[test]
fn visible_work_uses_lifo_before_normal_and_background_work() {
    let pool = Scheduler::new(1).unwrap();
    let (started, ready) = unbounded();
    let (unblock, gate) = unbounded();
    pool.schedule(Priority::Visible, LaneKey::Volume(vec![1]), 1, "gate", move |_| {
        started.send(()).unwrap();
        let _ = gate.recv_timeout(DEADLINE);
    });
    ready.recv_timeout(DEADLINE).unwrap();
    let (done, completed) = unbounded();
    for (priority, label) in [(Priority::Background, "background"), (Priority::Visible, "old visible"), (Priority::Normal, "normal"), (Priority::Visible, "new visible")] {
        let done = done.clone();
        pool.schedule(priority, LaneKey::Volume(vec![1]), 1, label, move |_| {
            done.send(label).unwrap();
        });
    }
    unblock.send(()).unwrap();
    let actual: Vec<_> = (0..4).map(|_| completed.recv_timeout(DEADLINE).unwrap()).collect();
    assert_eq!(actual, ["new visible", "old visible", "normal", "background"]);
}

#[test]
fn a_third_job_for_one_server_does_not_take_another_servers_worker() {
    let pool = Scheduler::new(3).unwrap();
    let (entered, running) = unbounded();
    let (unblock, gate) = unbounded();
    for label in ["first", "second", "third"] {
        let (entered, gate) = (entered.clone(), gate.clone());
        pool.schedule(Priority::Visible, LaneKey::Server(b"server-a".to_vec()), 2, label, move |_| {
            entered.send(label).unwrap();
            let _ = gate.recv_timeout(DEADLINE);
        });
    }
    let first = running.recv_timeout(DEADLINE).unwrap();
    let second = running.recv_timeout(DEADLINE).unwrap();
    let other = entered.clone();
    pool.schedule(Priority::Normal, LaneKey::Server(b"server-b".to_vec()), 2, "other server", move |_| {
        other.send("other server").unwrap();
    });
    assert_eq!(running.recv_timeout(DEADLINE).unwrap(), "other server");
    assert!(running.try_recv().is_err());
    unblock.send(()).unwrap();
    let remaining = running.recv_timeout(DEADLINE).unwrap();
    let mut names = [first, second, remaining];
    names.sort_unstable();
    assert_eq!(names, ["first", "second", "third"]);
    unblock.send(()).unwrap();
    unblock.send(()).unwrap();
}

#[test]
fn the_scheduler_logs_a_slow_task_and_starts_a_replacement_without_repeating_it() {
    let (log, logs) = unbounded();
    let pool = Scheduler::start(
        1,
        Duration::from_millis(40),
        Duration::from_millis(300),
        Box::new(move |label, _| {
            log.send(label.to_owned()).unwrap();
        }),
    )
    .unwrap();
    let (ready, started) = unbounded();
    let (unblock, gate) = unbounded();
    pool.schedule(Priority::Background, LaneKey::Volume(vec![1]), 1, "blocked", move |_| {
        ready.send(()).unwrap();
        let _ = gate.recv_timeout(DEADLINE);
    });
    started.recv_timeout(DEADLINE).unwrap();
    let (done, completed) = unbounded();
    pool.schedule(Priority::Visible, LaneKey::Volume(vec![2]), 1, "visible", move |_| {
        done.send(()).unwrap();
    });
    assert_eq!(logs.recv_timeout(DEADLINE).unwrap(), "blocked");
    assert!(completed.recv_timeout(Duration::from_millis(100)).is_err(), "logging alone must not free a worker");
    completed.recv_timeout(DEADLINE).unwrap();
    assert!(logs.try_recv().is_err());
    unblock.send(()).unwrap();
}

#[test]
fn one_spinning_volume_keeps_its_slot_even_if_a_worker_stalls() {
    let pool = Scheduler::start(1, Duration::from_millis(20), Duration::from_millis(80), Box::new(|_, _| {})).unwrap();
    let (ready, started) = unbounded();
    let (unblock, gate) = unbounded();
    pool.schedule(Priority::Normal, LaneKey::Volume(vec![1]), 1, "slow volume", move |_| {
        ready.send(()).unwrap();
        let _ = gate.recv_timeout(DEADLINE);
    });
    started.recv_timeout(DEADLINE).unwrap();
    let (done, completed) = unbounded();
    let other = done.clone();
    pool.schedule(Priority::Visible, LaneKey::Volume(vec![1]), 1, "same volume", move |_| {
        done.send("same").unwrap();
    });
    pool.schedule(Priority::Normal, LaneKey::Volume(vec![2]), 1, "other volume", move |_| {
        other.send("other").unwrap();
    });
    assert_eq!(completed.recv_timeout(DEADLINE).unwrap(), "other");
    assert!(completed.try_recv().is_err());
    unblock.send(()).unwrap();
    assert_eq!(completed.recv_timeout(DEADLINE).unwrap(), "same");
}

#[test]
fn a_long_scan_that_keeps_reporting_progress_does_not_get_a_replacement() {
    let (log, logs) = unbounded();
    let pool = Scheduler::start(
        1,
        Duration::from_millis(20),
        Duration::from_millis(200),
        Box::new(move |label, _| {
            log.send(label.to_owned()).unwrap();
        }),
    )
    .unwrap();
    let (unblock, gate) = unbounded();
    pool.schedule(Priority::Visible, LaneKey::Volume(vec![1]), 1, "working", move |progress| {
        while gate.recv_timeout(Duration::from_millis(10)).is_err_and(|e| e.is_timeout()) {
            progress.record();
        }
    });
    let (done, completed) = unbounded();
    pool.schedule(Priority::Background, LaneKey::Volume(vec![2]), 1, "waiting", move |_| {
        done.send(()).unwrap();
    });
    assert_eq!(logs.recv_timeout(DEADLINE).unwrap(), "working");
    assert!(completed.recv_timeout(Duration::from_millis(400)).is_err());
    unblock.send(()).unwrap();
    completed.recv_timeout(DEADLINE).unwrap();
}
