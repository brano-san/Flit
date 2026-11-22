use super::*;

fn pair(key: &str, val: Arg) -> Arg {
    Arg::L(vec![Arg::S(key.into()), Arg::L(vec![val])])
}

fn byte_array(bytes: &[u8]) -> Arg {
    Arg::L(bytes.iter().chain(std::iter::once(&0)).map(|&b| Arg::I(b.into())).collect())
}

fn expect_uris(value: Vec<V>, expected: &[&str]) {
    let [V::U(0), V::Props(props)] = value.as_slice() else { panic!("expected FileChooser success: {value:?}") };
    let Some((_, V::Strs(uris))) = props.iter().find(|(key, _)| key == "uris") else { panic!("missing URIs") };
    assert!(uris.iter().map(String::as_str).eq(expected.iter().copied()));
}

#[test]
fn open_decodes_filter_options_and_escapes_the_response_uri() {
    let current = Arg::L(vec![Arg::S("Images".into()), Arg::L(vec![Arg::L(vec![Arg::I(0), Arg::S("*.[pP][nN][gG]".into())])])]);
    let options = [pair("current_folder", byte_array(b"/tmp/images")), pair("accept_label", Arg::S("_Attach".into())), pair("current_filter", current.clone()), pair("filters", Arg::L(vec![current]))];
    let call = parse("OpenFile", "Attach an image", &options, Some).unwrap();
    assert_eq!(call.request.path.as_deref(), Some(b"/tmp/images".as_slice()));
    assert_eq!(call.request.accept.as_deref(), Some("Attach"));
    assert!(call.request.filters[0].matches(b"image.PnG"));
    assert!(!call.request.filters[0].matches(b"image.jpg"));
    let picked = (vec![b"/tmp/images/a b#%.png".to_vec()], 0, Vec::new());
    let reply = results(&call, picked, |_| Ok(false));
    assert!(matches!(&reply[1], V::Props(props) if props.iter().any(|(k, _)| k == "current_filter")));
    expect_uris(reply, &["file:///tmp/images/a%20b%23%25.png"]);
}

#[test]
fn save_file_uses_the_existing_file_name_and_save_files_picks_unique_paths() {
    let call = parse("SaveFile", "", &[pair("current_file", byte_array(b"/tmp/test.txt"))], Some).unwrap();
    assert_eq!(call.request.mode, Mode::Save(b"test.txt".to_vec()));
    assert_eq!(call.request.path.as_deref(), Some(b"/tmp".as_slice()));
    let picked = (vec![b"/tmp/test.txt".to_vec()], 0, Vec::new());
    expect_uris(results(&call, picked, |_| Ok(true)), &["file:///tmp/test.txt"]);

    let names = Arg::L(vec![byte_array(b"test.txt"), byte_array(b"test.txt")]);
    let call = parse("SaveFiles", "", &[pair("files", names)], Some).unwrap();
    let picked = (vec![b"/tmp".to_vec()], 0, Vec::new());
    let reply = results(&call, picked, |p| Ok(p.as_bytes() == b"/tmp/test.txt"));
    expect_uris(reply, &["file:///tmp/test%20%281%29.txt", "file:///tmp/test%20%282%29.txt"]);
}

#[test]
fn cancel_has_no_results_and_save_files_rejects_relative_paths() {
    assert!(matches!(response(1).as_slice(), [V::U(1), V::Props(props)] if props.is_empty()));
    let names = Arg::L(vec![byte_array(b"../outside")]);
    assert!(parse("SaveFiles", "", &[pair("files", names)], Some).is_none());
}

#[test]
fn idle_backend_releases_its_name_and_drains_a_queued_request() {
    use std::cell::RefCell;
    let events = RefCell::new(Vec::new());
    let mut calls = 0;
    serve_until_idle(Duration::ZERO, || events.borrow_mut().push("release"), |timeout| {
        calls += 1;
        match calls {
            1 => {
                assert_eq!(timeout, 0);
                events.borrow_mut().push("request");
                Some((true, true))
            }
            2 => {
                assert_eq!(timeout, 80);
                events.borrow_mut().push("reply");
                Some((false, true))
            }
            _ => panic!("must exit after the queued request has completed"),
        }
    });
    assert_eq!(events.into_inner(), ["release", "request", "reply"]);
}

#[test]
fn active_backend_keeps_its_name() {
    let mut timeouts = Vec::new();
    serve_until_idle(Duration::MAX, || panic!("must retain the name"), |timeout| {
        timeouts.push(timeout);
        (timeouts.len() == 1).then_some((true, true))
    });
    assert_eq!(timeouts, [1000, 80]);
}


#[test]
fn open_preserves_mime_rules_for_the_picker_and_the_response() {
    let current = Arg::L(vec![Arg::S("Text".into()), Arg::L(vec![Arg::L(vec![Arg::I(1), Arg::S("text/plain".into())])])]);
    let call = parse("OpenFile", "", &[pair("current_filter", current)], Some).unwrap();
    let filter = &call.request.filters[0];
    assert!(filter.patterns.is_empty());
    assert_eq!(filter.mime_types, ["text/plain"]);
    let request: Request = serde_json::from_slice(&serde_json::to_vec(&call.request).unwrap()).unwrap();
    assert_eq!(request.filters[0], *filter);
    let picked = (vec![b"/tmp/README".to_vec()], 0, Vec::new());
    let reply = results(&call, picked, |_| Ok(false));
    assert!(matches!(&reply[1], V::Props(props) if props.iter().any(|(k, _)| k == "current_filter")));
    expect_uris(reply, &["file:///tmp/README"]);
}

#[test]
fn the_any_mime_rule_does_not_require_classifying_files() {
    let rule = |media: &str| Arg::L(vec![Arg::I(1), Arg::S(media.into())]);
    let current = Arg::L(vec![Arg::S("Any".into()), Arg::L(vec![rule("*/*"), rule("text/plain")])]);
    let call = parse("OpenFile", "", &[pair("current_filter", current)], Some).unwrap();
    let filter = &call.request.filters[0];
    assert!(filter.mime_types.is_empty());
    assert!(filter.matches(b"plain-name"));
    assert!(filter.matches(b"image.png"));
}

#[test]
fn open_exposes_files_when_the_requested_type_has_no_matcher() {
    let current = Arg::L(vec![Arg::S("Unknown".into()), Arg::L(vec![Arg::L(vec![Arg::I(1), Arg::S("application/x-unknown".into())])])]);
    let call = parse("OpenFile", "", &[pair("current_filter", current)], |_| None).unwrap();
    let filter = &call.request.filters[0];
    assert!(filter.patterns.is_empty());
    assert!(filter.mime_types.is_empty());
    assert!(filter.matches(b"plain-name"));
    assert!(filter.matches(b"image.png"));
}
