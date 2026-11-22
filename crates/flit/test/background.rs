use super::*;

#[derive(Default, Clone)]
struct TestInstance(Arc<Mutex<Option<(OpenSink, Wake)>>>);

thread_local! {
    static TRANSPORT: TestInstance = TestInstance::default();
}

impl TestInstance {
    fn launch(&self, open: Open) -> Instance<Self> {
        let Some((sink, wake)) = self.0.lock().unwrap().clone() else { return Instance::Primary(self.clone()) };
        let sink = sink.lock().unwrap();
        if let Some(tx) = sink.as_ref() {
            tx.send(open).unwrap();
            wake();
            Instance::Forwarded
        } else {
            Instance::Standalone(PlatformError::new(0, "closed listener"))
        }
    }
}

impl SingleInstance for TestInstance {
    fn acquire(open: &Open) -> Instance<Self> {
        TRANSPORT.with(|transport| transport.launch(open.clone()))
    }

    fn listen(self, opens: OpenSink, wake: Wake) {
        *self.0.lock().unwrap() = Some((opens, wake));
    }
}

#[test]
fn a_second_launch_wakes_the_background_instance_and_requests_its_window() {
    use flit_core::ui::background::{idle, Idle};

    let Instance::Primary(instance) = TestInstance::acquire_background() else { panic!("expected the first process") };
    let woke = Arc::new(AtomicBool::new(false));
    let flag = woke.clone();
    let (sink, opens) = open_inbox(Some(instance), Arc::new(move || flag.store(true, Ordering::Relaxed)));
    assert_eq!(idle(true, Some(true), false, !opens.is_empty()), Idle::Wait);

    let open = Open::location(&Location::parse("/tmp").unwrap(), OpenTarget::NewWindow);
    assert!(matches!(TestInstance::acquire(&open), Instance::Forwarded));
    assert!(woke.load(Ordering::Relaxed));
    assert_eq!(idle(true, Some(true), false, !opens.is_empty()), Idle::Open);
    assert_eq!(opens.try_recv().unwrap(), open);
    assert_eq!(idle(true, Some(true), true, false), Idle::Exit);
    sink.lock().unwrap().take();
    assert!(matches!(TestInstance::acquire(&Open::default()), Instance::Standalone(_)));
}

#[test]
fn releasing_the_graphics_resources_drops_devices_and_disconnects_late_replies() {
    let hw = Arc::new(());
    let sw = Arc::new(());
    let (tx, rx) = unbounded();
    let mut graphics = Graphics { replies: rx, hw: Some(hw.clone()), sw: Some(sw.clone()), pending: 1 };
    assert_eq!(graphics.pending, 1);
    graphics.release();
    assert_eq!((Arc::strong_count(&hw), Arc::strong_count(&sw)), (1, 1));
    assert!(tx.send((false, Ok(hw.clone()))).is_err());
    assert!(graphics.hw.is_none() && graphics.sw.is_none());

    let (tx, rx) = unbounded();
    graphics.replies = rx;
    tx.send((true, Ok(sw))).unwrap();
    assert!(graphics.replies.try_recv().unwrap().0);
}
