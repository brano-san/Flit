use super::*;

#[test]
fn mpv_events_follow_the_position_pause_and_end_of_the_media() {
    let mut p = Progress { playing: true, ..Progress::default() };
    assert!(p.apply(r#"{"event":"property-change","id":2,"name":"duration","data":61.5}"#));
    assert!(p.apply(r#"{"event":"property-change","id":1,"name":"time-pos","data":12.25}"#));
    assert_eq!((p.position, p.duration, p.playing), (12.25, 61.5, true));
    assert!(p.apply(r#"{"event":"property-change","id":3,"name":"pause","data":true}"#));
    assert!(!p.playing, "pausing in the player's own window turns the Play button back");
    assert!(p.apply(r#"{"event":"end-file","reason":"eof"}"#));
    assert_eq!((p.position, p.playing, p.ended), (61.5, false, true), "the end shows a full bar and a Play button");
    assert_eq!(p.event(7), HostEvent::Media { gen: 7, position: 61.5, duration: 61.5, playing: false });
}

#[test]
fn unrelated_or_broken_mpv_lines_change_nothing() {
    let mut p = Progress { position: 3.0, duration: 9.0, playing: true, ended: false };
    let before = p;
    for line in ["", "not json", r#"{"event":"property-change","name":"time-pos","data":null}"#, r#"{"request_id":0,"error":"success"}"#, r#"{"event":"playback-restart"}"#, r#"{"event":"property-change","name":"volume","data":50}"#] {
        assert!(!p.apply(line), "{line}");
    }
    assert_eq!(p, before);
    let mut unknown = Progress { playing: true, ..Progress::default() };
    unknown.finish();
    assert_eq!((unknown.position, unknown.playing), (0.0, false), "an end without a known length keeps the bar empty");
}

#[test]
fn library_property_changes_move_the_same_bar_as_mpv_messages() {
    let mut p = Progress { playing: true, ..Progress::default() };
    assert!(p.change("duration", Some(30.0), None));
    assert!(p.change("time-pos", Some(-0.5), None));
    assert!(p.change("pause", None, Some(true)));
    assert_eq!((p.position, p.duration, p.playing), (0.0, 30.0, false));
    assert!(!p.change("pause", Some(1.0), None), "a pause without a flag is ignored");
    assert!(!p.change("time-pos", None, None), "a position mpv cannot tell is ignored");
    assert!(!p.change("volume", Some(80.0), None));
    assert_eq!((p.position, p.duration, p.playing), (0.0, 30.0, false));
}

#[test]
fn every_preview_control_becomes_the_same_order_for_both_players() {
    assert_eq!(order(MediaCmd::Play), Order::Pause(false));
    assert_eq!(order(MediaCmd::Pause), Order::Pause(true));
    assert_eq!(order(MediaCmd::Seek(12.5)), Order::Seek(12.5));
    assert_eq!(order(MediaCmd::Seek(-3.0)), Order::Seek(0.0), "a drag left of the bar seeks to the start");
    assert_eq!(order(MediaCmd::Volume(0.375)), Order::Volume(38.0));
    assert_eq!(order(MediaCmd::Volume(4.0)), Order::Volume(100.0));
    assert_eq!(order(MediaCmd::Volume(-1.0)), Order::Volume(0.0));
    assert_eq!(stop_signal(Order::Pause(true)), Some(libc::SIGSTOP), "players without a control socket are paused by a signal");
    assert_eq!(stop_signal(Order::Pause(false)), Some(libc::SIGCONT));
    assert_eq!(stop_signal(Order::Seek(1.0)), None, "a seek is never turned into a signal");
}

#[test]
fn player_commands_are_one_json_line_each() {
    assert_eq!(line(order(MediaCmd::Seek(12.5))), "{\"command\":[\"seek\",12.5,\"absolute\"]}\n");
    assert_eq!(line(order(MediaCmd::Seek(-3.0))), "{\"command\":[\"seek\",0.0,\"absolute\"]}\n");
    assert_eq!(line(order(MediaCmd::Volume(0.375))), "{\"command\":[\"set_property\",\"volume\",38.0]}\n");
    assert_eq!(line(order(MediaCmd::Volume(4.0))), "{\"command\":[\"set_property\",\"volume\",100.0]}\n");
    assert_eq!(line(order(MediaCmd::Pause)), "{\"command\":[\"set_property\",\"pause\",true]}\n");
    assert_eq!(observe().lines().count(), 3);
    assert!(observe().contains("\"time-pos\"") && observe().contains("\"pause\""));
    let s = socket_path(Path::new("/run/user/1000"), 42);
    assert!(s.starts_with("/run/user/1000") && s.to_string_lossy().ends_with("-42.sock"));
    assert_eq!(crate::shell::mpv_args(Some(&s), Some(0.5)), ["--no-terminal".to_owned(), format!("--input-ipc-server={}", s.display()), "--volume=50".to_owned()]);
    assert_eq!(crate::shell::mpv_args(Some(&s), None).len(), 2, "without a known volume mpv keeps its own");
}

#[test]
fn without_a_private_runtime_folder_mpv_gets_no_control_socket() {
    assert_eq!(crate::shell::mpv_args(None, None), ["--no-terminal".to_owned()]);
}

#[test]
fn the_library_player_is_tried_unless_it_is_known_to_be_missing() {
    assert_eq!(backend(Some(true)), Backend::Embedded);
    assert_eq!(backend(None), Backend::Embedded, "the library is looked for off the window thread on the first play");
    assert_eq!(backend(Some(false)), Backend::External, "without libmpv the separate mpv window and the other players stay");
}

#[test]
fn frames_follow_the_refresh_rate_of_the_screen() {
    let hz = |d: Duration| (1.0 / d.as_secs_f64()).round();
    assert_eq!(hz(frame_interval(Some(144_000))), 144.0);
    assert_eq!(hz(frame_interval(Some(59_940))), 60.0);
    assert_eq!(hz(frame_interval(None)), 60.0, "an unknown screen gets 60 frames a second");
    assert_eq!(hz(frame_interval(Some(1))), 24.0, "a broken refresh rate never stalls the video");
    assert_eq!(hz(frame_interval(Some(10_000_000))), 360.0);
}

#[test]
fn the_video_area_is_taken_from_the_placed_rectangle() {
    let r = |w: f32, h: f32| flit_core::render::Rect::new(10.0, 20.0, w, h);
    assert_eq!(area(r(640.4, 359.6)), Some((640, 360)));
    assert_eq!(area(r(0.0, 300.0)), None, "a collapsed pane keeps the previous size");
    assert_eq!(area(r(f32::NAN, 300.0)), None);
    assert_eq!(area(r(-5.0, 300.0)), None);
}

#[test]
#[ignore = "plays user-hlg.mp4 with libmpv when FLIT_HDR_FIXTURES is set"]
fn user_hlg_playback_honours_the_preview_controls_and_releases_the_file() {
    let Some(directory) = std::env::var_os("FLIT_HDR_FIXTURES") else { return };
    let path = Path::new(&directory).join("user-hlg.mp4").canonicalize().unwrap();
    let (tx, events) = std::sync::mpsc::channel();
    let start = Start { path: path.clone(), video: true, gen: 8, volume: Some(0.0), area: Some((1280, 720)), every: frame_interval(None) };
    let player = crate::mpv::Embedded::start(start, Arc::new(move |e| {
        let _ = tx.send(e);
    }));
    let mut first = None;
    let mut frames = 0;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let event = events.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("no preview frames");
        if let HostEvent::Frame(slot) = event {
            let Some((_, frame)) = slot.take() else { continue };
            let elapsed = first.get_or_insert_with(Instant::now).elapsed();
            frames += 1;
            if elapsed >= Duration::from_secs(8) {
                assert!(frame.rgba.chunks_exact(4).any(|pixel| pixel[1] > 16));
                println!("user-hlg: {frames} preview frames in {elapsed:?}, {:.2} fps", f64::from(frames - 1) / elapsed.as_secs_f64());
                break;
            }
        }
    }
    let wait_for = |expected: &dyn Fn(f64, f64, bool) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            match events.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("preview playback timed out") {
                HostEvent::Frame(slot) => { slot.take(); }
                HostEvent::Media { position, duration, playing, .. } if duration > 0.0 && expected(position, duration, playing) => break duration,
                HostEvent::Failed { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
    };
    let duration = wait_for(&|position, _, playing| playing && position > 0.5);
    assert!(player.order(Order::Pause(true)));
    wait_for(&|position, duration, playing| !playing && position < duration);
    assert!(player.order(Order::Volume(30.0)));
    assert!(player.order(Order::Seek(duration - 3.0)));
    assert!(player.order(Order::Pause(false)));
    wait_for(&|position, duration, playing| playing && position >= duration - 4.0);
    wait_for(&|position, duration, playing| !playing && position == duration);
    assert!(!player.order(Order::Pause(false)), "EOF ends the session");
    player.stop().unwrap().join().unwrap();
    let mut descriptors = std::fs::read_dir("/proc/self/fd").unwrap().flatten();
    assert!(!descriptors.any(|fd| fd.path().read_link().is_ok_and(|target| target == path)), "closing the preview releases the video");
}
