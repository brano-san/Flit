use crate::listing::EntryBatch;
use crate::provider::{Event, Request};
use crate::transport::{read_frame_limited, read_hello, Hello, Protocol};
use proptest::prelude::*;

const LIMIT: usize = 1 << 16;

proptest! {
    #![proptest_config(ProptestConfig { cases: std::env::var("FLIT_FUZZ_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(256), failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn hello_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = read_hello(&mut bytes.as_slice(), &Hello::new(Protocol::Index, 2));
    }

    #[test]
    fn frames_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let mut buf = Vec::new();
        if let Ok(batch) = read_frame_limited::<_, EntryBatch>(&mut bytes.as_slice(), &mut buf, LIMIT) {
            let _ = batch.entries(true).map(|it| it.count());
            let _ = batch.entries(false).map(|it| it.count());
        }
        let _ = read_frame_limited::<_, Request>(&mut bytes.as_slice(), &mut buf, LIMIT);
        let _ = read_frame_limited::<_, Event>(&mut bytes.as_slice(), &mut buf, LIMIT);
    }

    #[test]
    fn oversized_length_is_rejected_before_allocation(len in (LIMIT as u32 + 1).., tail in proptest::collection::vec(any::<u8>(), 0..8)) {
        let mut bytes = len.to_le_bytes().to_vec();
        bytes.extend(tail);
        let mut buf = Vec::new();
        prop_assert!(read_frame_limited::<_, EntryBatch>(&mut bytes.as_slice(), &mut buf, LIMIT).is_err());
        prop_assert!(buf.capacity() <= LIMIT);
    }

    #[test]
    fn batches_with_lying_lengths_are_rejected(names in proptest::collection::vec(any::<u8>(), 0..32), lens in proptest::collection::vec(any::<u32>(), 0..8)) {
        let entries = lens.iter().map(|&name_len| crate::listing::WireEntry { name_len, meta: Default::default() }).collect();
        let batch = EntryBatch { names, entries };
        let _ = batch.entries(true).map(|it| it.count());
    }
}
