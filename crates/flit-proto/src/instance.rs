use flit_core::Open;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 2;

pub const MAX_FRAME: usize = 256 << 10;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Open(Open),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Accepted,
}

#[cfg(test)]
#[path = "../test/instance.rs"]
mod tests;
