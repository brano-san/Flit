use crate::listing::{check_meta, check_name, EntryBatch};
use crate::transport::ProtoError;
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

impl Event {
    pub fn validate(&self, windows_names: bool) -> Result<(), ProtoError> {
        match self {
            Event::Entries { batch, .. } => batch.entries(windows_names).map(drop),
            Event::Changes { changes, .. } => changes.iter().try_for_each(|c| {
                check_name(&c.name, windows_names)?;
                c.meta.as_ref().map_or(Ok(()), check_meta)
            }),
            Event::Stat { result: Ok(meta), .. } => check_meta(meta),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "../test/provider.rs"]
mod tests;
