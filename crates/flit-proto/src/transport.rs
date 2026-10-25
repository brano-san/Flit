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
            ProtoError::TooLarge(n) => write!(f, "message of {n} bytes exceeds the {MAX_FRAME} byte limit"),
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
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
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
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Duplex {
        rx: Cursor<Vec<u8>>,
        tx: Vec<u8>,
    }
    impl Read for Duplex {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.rx.read(b)
        }
    }
    impl Write for Duplex {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.tx.write(b)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn peer_says(frames: &[Hello]) -> Duplex {
        let mut rx = Vec::new();
        for f in frames {
            write_frame(&mut rx, f).unwrap();
        }
        Duplex { rx: Cursor::new(rx), tx: Vec::new() }
    }

    #[test]
    fn handshake_ok_and_sends_ours() {
        let ours = Hello::new(Protocol::Provider, 3);
        let mut s = peer_says(&[Hello::new(Protocol::Provider, 3)]);
        assert_eq!(handshake(&mut s, &ours).unwrap().version, 3);
        let sent: Hello = read_frame(&mut Cursor::new(s.tx), &mut Vec::new()).unwrap();
        assert_eq!(sent, ours);
    }

    #[test]
    fn handshake_errors_are_specific() {
        let ours = Hello::new(Protocol::Provider, 3);
        let old = Hello { build: "0.0.9".into(), ..Hello::new(Protocol::Provider, 2) };
        let err = handshake(&mut peer_says(&[old]), &ours).unwrap_err();
        assert!(matches!(err, ProtoError::VersionMismatch { ours: 3, theirs: 2, .. }));
        assert!(err.to_string().contains("0.0.9") && err.to_string().contains("v2"));
        let wrong = Hello::new(Protocol::Index, 3);
        assert!(matches!(handshake(&mut peer_says(&[wrong]), &ours), Err(ProtoError::NotFlit)));
        let text: Vec<u8> = "Error: distro not found\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut s = Duplex { rx: Cursor::new(text), tx: Vec::new() };
        assert!(matches!(handshake(&mut s, &ours), Err(ProtoError::NotFlit)));
    }

    #[test]
    fn hostile_frames() {
        let mut buf = Vec::new();
        let r: Result<Hello, _> = read_frame(&mut Cursor::new(u32::MAX.to_le_bytes().to_vec()), &mut buf);
        assert!(matches!(r, Err(ProtoError::TooLarge(_))));
        let r: Result<Hello, _> = read_frame(&mut Cursor::new(vec![10, 0, 0, 0, 1, 2]), &mut buf);
        assert!(matches!(r, Err(ProtoError::Io(_))));
        let r: Result<Hello, _> = read_frame(&mut Cursor::new(vec![2, 0, 0, 0, 0xFF, 0xFF]), &mut buf);
        assert!(matches!(r, Err(ProtoError::Decode(_))));
    }
}
