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

#[test]
fn limited_frames() {
    let mut wire = Vec::new();
    write_frame(&mut wire, &vec![7u8; 100]).unwrap();
    let r: Result<Vec<u8>, _> = read_frame_limited(&mut Cursor::new(wire.clone()), &mut Vec::new(), 64);
    assert!(matches!(r, Err(ProtoError::TooLarge(_))));
    let r: Vec<u8> = read_frame_limited(&mut Cursor::new(wire), &mut Vec::new(), 128).unwrap();
    assert_eq!(r, vec![7u8; 100]);
}
