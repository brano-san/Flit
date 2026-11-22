use crate::i18n::S;
use crate::path::FsPath;
use crate::provider::OpCtl;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

pub const ELEVATED_FLAG: &str = "--read-disk-changes";
pub const MAX_ROWS: usize = 10_000;
pub const HOUR: i64 = 36_000_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Filter {
    pub since: i64,
    pub until: i64,
    pub name: String,
    pub created: bool,
    pub deleted: bool,
    pub renamed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    Created,
    Deleted,
    RenameFrom,
    RenameTo,
}

impl ChangeKind {
    pub fn key(self) -> S {
        match self {
            Self::Created => S::DiskChangesCreated,
            Self::Deleted => S::DiskChangesDeleted,
            Self::RenameFrom => S::DiskChangesRenameFrom,
            Self::RenameTo => S::DiskChangesRenameTo,
        }
    }

    fn csv(self) -> &'static str {
        match self {
            Self::Created => "create",
            Self::Deleted => "delete",
            Self::RenameFrom => "rename_from",
            Self::RenameTo => "rename_to",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub at: i64,
    pub kind: ChangeKind,
    pub name: String,
    pub id: u64,
    pub parent: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub changes: Vec<Change>,
    pub truncated: bool,
}

impl Report {
    pub fn csv(&self) -> String {
        let mut text = String::from("\u{feff}time_utc,filetime_utc,event,name,file_id,parent_id\r\n");
        let mut time = String::new();
        for c in &self.changes {
            time.clear();
            crate::fmt::datetime(&mut time, c.at, 0);
            let name = escape_csv(&c.name);
            let _ = write!(text, "{time},{},{},\"{name}\",{},{}\r\n", c.at, c.kind.csv(), c.id, c.parent);
        }
        text
    }
}

fn escape_csv(value: &str) -> String {
    let prefix = if value.trim_start().starts_with(['=', '+', '-', '@']) || value.starts_with(['\t', '\r', '\n']) { "'" } else { "" };
    format!("{prefix}{}", value.replace('"', "\"\""))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Error {
    Denied,
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
pub enum Request {
    Read { root: FsPath, filter: Filter, elevated: bool, ctl: OpCtl },
    Export { path: FsPath, report: Report, ctl: OpCtl },
}

#[derive(Debug)]
pub enum Reply {
    Read(Result<Report, Error>),
    Exported(Result<FsPath, Error>),
}

#[cfg(test)]
#[path = "../test/disk_changes.rs"]
mod tests;
