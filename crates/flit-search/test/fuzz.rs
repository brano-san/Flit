use crate::proto::{Message, Sort, SortKey};
use crate::query::{glob, parse, shell_query, Clock};
use flit_proto::transport::{encode, read_frame_limited};
use proptest::prelude::*;

const QUERY_ALPHABET: &str = "[ a-zA-Z0-9:!\"*?.;<>=/\\\\\\-$()\\[\\]{}|+^\u{0}\u{7f}\u{e9}\u{1f600}]{0,64}";

fn clock() -> Clock {
    Clock { now: 133_000_000_000_000_000, utc_offset_min: 180 }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: std::env::var("FLIT_FUZZ_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(256), failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn parse_never_panics(text in QUERY_ALPHABET) {
        let _ = parse(&text, clock());
        let _ = shell_query(&text);
    }

    #[test]
    fn parse_never_panics_on_any_string(text in any::<String>()) {
        let _ = parse(&text, clock());
    }

    #[test]
    fn glob_never_panics(p in proptest::collection::vec(any::<u8>(), 0..16), s in proptest::collection::vec(any::<u8>(), 0..32)) {
        let _ = glob(&p, &s);
    }

    #[test]
    fn request_frames_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let mut buf = Vec::new();
        if let Ok(Message::Rows { entries, .. }) = read_frame_limited::<_, Message>(&mut bytes.as_slice(), &mut buf, 64 << 10) {
            let _ = entries.entries(true).map(|it| it.count());
        }
    }

    #[test]
    fn query_messages_round_trip(gen in any::<u64>(), text in any::<String>(), from in any::<u64>(), to in any::<u64>()) {
        let sort = Sort { key: SortKey::Path, descending: true };
        for msg in [Message::Query { gen, text: text.clone(), sort }, Message::GetRows { gen, from, to }] {
            let mut out = Vec::new();
            encode(&msg, &mut out).unwrap();
            let back: Message = read_frame_limited(&mut out.as_slice(), &mut Vec::new(), out.len()).unwrap();
            prop_assert_eq!(back, msg);
        }
    }
}

#[test]
fn regression_inputs_parse_without_panicking() {
    for text in ["regex:", "\"", "size:", "size:..", "dm:..", "dm:9999-12-31", "dm:+999", "size:1e400", "ext:", "!!!", "NOT NOT", "path:regex:(", "dm:2024-02-31"] {
        let _ = parse(text, clock());
    }
}
