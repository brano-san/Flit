use crate::listing::EntryBatch;
use flit_core::listing::DirChange;
use flit_core::provider::FileOp;
use flit_core::{FsError, FsPath, Meta};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

pub type ReqId = u64;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Request {
    List { id: ReqId, path: FsPath },
    Stat { id: ReqId, path: FsPath },
    Watch { id: ReqId, path: FsPath },
    DirSizes { id: ReqId, dir: FsPath, items: Vec<(u32, Vec<u8>)> },
    Op { id: ReqId, op: FileOp },
    Cancel { id: ReqId },
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Event {
    Entries { id: ReqId, batch: EntryBatch },
    ListDone { id: ReqId, error: Option<FsError> },
    Stat { id: ReqId, result: Result<Meta, FsError> },
    Changes { id: ReqId, changes: Vec<DirChange> },
    Rescan { id: ReqId },
    DirSize { id: ReqId, idx: u32, size: u64 },
    OpDone { id: ReqId, error: Option<FsError> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{decode, encode};
    use flit_core::listing::ChangeKind;
    use flit_core::FsErrorKind;

    fn unix(p: &[u8]) -> FsPath {
        FsPath::parse_bytes(p).unwrap()
    }

    #[test]
    fn roundtrip_every_message() {
        let requests = vec![
            Request::List { id: 1, path: unix(b"/home/\xFFuser") },
            Request::Stat { id: 2, path: unix(b"/") },
            Request::Watch { id: 3, path: unix("/tmp/отчёт".as_bytes()) },
            Request::DirSizes { id: 4, dir: unix(b"/var"), items: vec![(0, b"log".to_vec()), (7, b"\xFE".to_vec())] },
            Request::Op { id: 5, op: FileOp::Trash(vec![unix(b"/tmp/a"), unix(b"/tmp/b:c")]) },
            Request::Cancel { id: 1 },
            Request::Shutdown,
        ];
        for m in requests {
            let mut buf = Vec::new();
            encode(&m, &mut buf).unwrap();
            assert_eq!(decode::<Request>(&buf[4..]).unwrap(), m);
        }
        let mut batch = EntryBatch::default();
        batch.push(b"\xFF\xFE", Meta { size: 1, attrs: flit_core::attr::HIDDEN, ..Default::default() });
        let err = FsError { kind: FsErrorKind::AccessDenied, code: 13 };
        let events = vec![
            Event::Entries { id: 1, batch },
            Event::ListDone { id: 1, error: Some(err.clone()) },
            Event::Stat { id: 2, result: Ok(Meta { size: 9, modified: -5, created: 1, attrs: 0 }) },
            Event::Stat { id: 2, result: Err(err) },
            Event::Changes { id: 3, changes: vec![DirChange { kind: ChangeKind::Removed, name: b"x".to_vec(), meta: None }] },
            Event::Rescan { id: 3 },
            Event::DirSize { id: 4, idx: 7, size: u64::MAX },
            Event::OpDone { id: 5, error: None },
        ];
        for m in events {
            let mut buf = Vec::new();
            encode(&m, &mut buf).unwrap();
            assert_eq!(decode::<Event>(&buf[4..]).unwrap(), m);
        }
    }

    #[test]
    fn forged_paths_rejected() {
        let mut buf = Vec::new();
        encode(&(0u8, 1u64, b"../../etc".to_vec()), &mut buf).unwrap();
        assert!(decode::<Request>(&buf[4..]).is_err());
    }
}
