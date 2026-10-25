use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{self, Read, Write};

pub const MAX_FRAME: usize = 16 << 20;
pub const MAGIC: [u8; 4] = *b"FLIT";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Index,
    Provider,
    Instance,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub magic: [u8; 4],
    pub protocol: Protocol,
    pub version: u32,
    pub build: String,
}

impl Hello {
    pub fn new(protocol: Protocol, version: u32) -> Self {
        Self { magic: MAGIC, protocol, version, build: env!("CARGO_PKG_VERSION").into() }
    }

    pub fn check(&self, theirs: &Hello) -> Result<(), ProtoError> {
        if theirs.magic != MAGIC || theirs.protocol != self.protocol {
            return Err(ProtoError::NotFlit);
        }
        if theirs.version != self.version {
            return Err(ProtoError::VersionMismatch { ours: self.version, theirs: theirs.version, build: theirs.build.clone() });
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum ProtoError {
    Io(io::Error),
    Decode(postcard::Error),
    TooLarge(usize),
    NotFlit,
    VersionMismatch { ours: u32, theirs: u32, build: String },
    Invalid(&'static str),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtoError::Io(e) => write!(f, "connection: {e}"),
            ProtoError::Decode(e) => write!(f, "malformed message: {e}"),
            ProtoError::TooLarge(n) => write!(f, "message of {n} bytes exceeds the size limit"),
            ProtoError::NotFlit => write!(f, "peer does not speak this Flit protocol"),
            ProtoError::VersionMismatch { ours, theirs, build } => {
                write!(f, "peer (Flit {build}) speaks protocol v{theirs}, this build speaks v{ours}")
            }
            ProtoError::Invalid(what) => write!(f, "invalid message: {what}"),
        }
    }
}

impl std::error::Error for ProtoError {}

impl From<io::Error> for ProtoError {
    fn from(e: io::Error) -> Self {
        ProtoError::Io(e)
    }
}

impl From<postcard::Error> for ProtoError {
    fn from(e: postcard::Error) -> Self {
        ProtoError::Decode(e)
    }
}

pub fn encode<T: Serialize>(msg: &T, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    let buf = std::mem::take(out);
    *out = postcard::to_extend(msg, buf)?;
    let len = out.len() - start - 4;
    if len > MAX_FRAME {
        out.truncate(start);
        return Err(ProtoError::TooLarge(len));
    }
    out[start..start + 4].copy_from_slice(&(len as u32).to_le_bytes());
    Ok(())
}

pub fn decode<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ProtoError> {
    Ok(postcard::from_bytes(payload)?)
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), ProtoError> {
    let mut buf = Vec::new();
    encode(msg, &mut buf)?;
    w.write_all(&buf)?;
    Ok(())
}

pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R, buf: &mut Vec<u8>) -> Result<T, ProtoError> {
    read_frame_limited(r, buf, MAX_FRAME)
}

pub fn read_frame_limited<R: Read, T: DeserializeOwned>(r: &mut R, buf: &mut Vec<u8>, max: usize) -> Result<T, ProtoError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max {
        return Err(ProtoError::TooLarge(len));
    }
    buf.resize(len, 0);
    r.read_exact(buf)?;
    decode(buf)
}

const MAX_HELLO: usize = 256;

pub fn read_hello<R: Read>(r: &mut R, ours: &Hello) -> Result<Hello, ProtoError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_HELLO {
        return Err(ProtoError::NotFlit);
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf)?;
    let theirs: Hello = decode(&buf).map_err(|_| ProtoError::NotFlit)?;
    ours.check(&theirs)?;
    Ok(theirs)
}

pub fn handshake<S: Read + Write>(s: &mut S, ours: &Hello) -> Result<Hello, ProtoError> {
    write_frame(s, ours)?;
    s.flush()?;
    read_hello(s, ours)
}

#[cfg(test)]
#[path = "../test/transport.rs"]
mod tests;
