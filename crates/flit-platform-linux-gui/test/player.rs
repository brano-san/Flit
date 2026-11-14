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
fn player_commands_are_one_json_line_each() {
    assert_eq!(seek(12.5), "{\"command\":[\"seek\",12.5,\"absolute\"]}\n");
    assert_eq!(seek(-3.0), "{\"command\":[\"seek\",0.0,\"absolute\"]}\n");
    assert_eq!(volume(0.375), "{\"command\":[\"set_property\",\"volume\",38.0]}\n");
    assert_eq!(volume(4.0), "{\"command\":[\"set_property\",\"volume\",100.0]}\n");
    assert_eq!(pause(true), "{\"command\":[\"set_property\",\"pause\",true]}\n");
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
