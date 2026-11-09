use super::*;

fn manifest(platforms: &str) -> String {
    format!(r#"{{"version":"0.4.0","notes":" Faster copies \n","pub_date":"2026-11-09T10:00:00Z","platforms":{{{platforms}}}}}"#)
}

#[test]
fn manifest_gives_the_release_for_this_platform() {
    let text = manifest(r#""windows-x86_64":{"url":"https://example.com/flit-0.4.0-x86_64-windows.zip","signature":"AAAA"},"linux-x86_64":{"url":"https://example.com/flit.tar.gz","signature":"QUJD"}"#);
    let win = parse_manifest(&text, WINDOWS).unwrap();
    assert_eq!(win, Release { version: "0.4.0".into(), notes: "Faster copies".into(), url: "https://example.com/flit-0.4.0-x86_64-windows.zip".into(), signature: "AAAA".into() });
    assert_eq!(parse_manifest(&text, LINUX).unwrap().url, "https://example.com/flit.tar.gz");
}

#[test]
fn manifest_without_this_platform_or_broken_is_an_error() {
    let only_linux = manifest(r#""linux-x86_64":{"url":"https://example.com/a.tar.gz","signature":"QUJD"}"#);
    let err = parse_manifest(&only_linux, WINDOWS).unwrap_err();
    assert!(err.contains("0.4.0") && err.contains(WINDOWS), "{err}");
    assert!(parse_manifest("not json", WINDOWS).is_err());
    assert!(parse_manifest(r#"{"version":"soon","platforms":{}}"#, WINDOWS).is_err());
    let plain_http = manifest(r#""windows-x86_64":{"url":"http://example.com/a.zip","signature":"QUJD"}"#);
    assert!(parse_manifest(&plain_http, WINDOWS).is_err(), "only https downloads");
    let bad_signature = manifest(r#""windows-x86_64":{"url":"https://example.com/a.zip","signature":"*!"}"#);
    assert!(parse_manifest(&bad_signature, WINDOWS).is_err());
}

#[test]
fn rendered_manifest_parses_back() {
    let text = render_manifest("1.2.3", "Notes\nline two", "2026-11-09T10:00:00Z", &[(WINDOWS, "https://x/a.zip".into(), "QUJD".into()), (LINUX, "https://x/a.tar.gz".into(), "REVG".into())]);
    let r = parse_manifest(&text, LINUX).unwrap();
    assert_eq!((r.version.as_str(), r.notes.as_str(), r.url.as_str(), r.signature.as_str()), ("1.2.3", "Notes\nline two", "https://x/a.tar.gz", "REVG"));
    assert!(text.contains("\"pub_date\": \"2026-11-09T10:00:00Z\""));
}

#[test]
fn assets_map_to_platform_keys() {
    assert_eq!(platform_of_asset("flit-0.4.0-x86_64-windows.zip"), Some(WINDOWS));
    assert_eq!(platform_of_asset("flit-0.4.0-x86_64-linux.tar.gz"), Some(LINUX));
    assert_eq!(platform_of_asset("flit_0.4.0_amd64.deb"), None);
}

#[test]
fn only_strictly_newer_versions_count() {
    assert!(newer("0.3.3", "0.3.2"));
    assert!(newer("0.10.0", "0.9.9"), "numeric, not text order");
    assert!(newer("1.0", "0.99.99"));
    assert!(newer("v0.4.0", "0.3.2"));
    assert!(newer("0.4.0", "0.4.0-rc.1"), "a release beats its pre-release");
    assert!(!newer("0.4.0-rc.1", "0.4.0"));
    assert!(!newer("0.3.2", "0.3.2"));
    assert!(!newer("0.3.2.0", "0.3.2"));
    assert!(!newer("0.3.1", "0.3.2"));
    assert!(!newer("garbage", "0.3.2"));
    assert!(!newer("1.0.0", "garbage"));
}

#[test]
fn base64_round_trips_and_rejects_noise() {
    for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0, 255, 128, 7]] {
        let text = base64_encode(bytes);
        assert_eq!(base64_decode(&text).as_deref(), Some(bytes), "{text}");
    }
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_decode("Zm8").as_deref(), Some(&b"fo"[..]));
    assert_eq!(base64_decode("Zm9v YmFy"), None);
    assert_eq!(base64_decode("Zm9=x"), None);
}

#[test]
fn percent_needs_a_known_total() {
    assert_eq!(percent(42, Some(100)), Some(42));
    assert_eq!(percent(150, Some(100)), Some(100));
    assert_eq!(percent(5, None), None);
    assert_eq!(percent(5, Some(0)), None);
}
