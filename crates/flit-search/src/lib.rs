pub mod proto;

pub mod index {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Copy)]
    pub struct IndexRecord<'a> {
        pub id: u64,
        pub parent: u64,
        pub name: &'a [u8],
        pub size: u64,
        pub modified: i64,
        pub attrs: u32,
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Checkpoint {
        pub journal: u64,
        pub next: i64,
    }

    pub trait IndexSink {
        fn upsert(&mut self, record: &IndexRecord<'_>);
        fn remove(&mut self, id: u64);
    }
}
