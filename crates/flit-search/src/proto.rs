use flit_proto::listing::EntryBatch;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 4;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Path,
    Size,
    Modified,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum VolumeState {
    Scanning { progress: f32 },
    Ready,
    Unsupported,
    Error(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VolumeInfo {
    pub id: String,
    pub label: String,
    pub file_system: String,
    pub files: u64,
    pub state: VolumeState,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Message {
    Query { gen: u64, text: String, conditions: Vec<String>, sort: Sort, paths: flit_core::query_attrs::SearchPaths },
    QueryDone { gen: u64, total: u64, version: u64 },
    Groups { gen: u64, starts: Vec<u64> },
    GetRows { gen: u64, from: u64, to: u64 },
    Rows { gen: u64, from: u64, entries: EntryBatch, dirs: Vec<Vec<u8>> },
    IndexStatus { volumes: Vec<VolumeInfo> },
    Error { message: String },
    GetFolders { gen: u64 },
    Folders { gen: u64, counts: flit_core::query_attrs::FolderCounts },
    GetFacets { gen: u64, open: bool },
    Facets { gen: u64, counts: flit_core::query_attrs::Facets },
}

#[cfg(test)]
#[path = "../test/proto.rs"]
mod tests;
