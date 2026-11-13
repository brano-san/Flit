use super::*;
use crate::transport::{read_frame, write_frame, Hello, Protocol};
use flit_core::open::{parse_args, OpenTarget};
use std::io::Cursor;

#[test]
fn open_roundtrip_with_hello() {
    let args: Vec<Vec<u8>> = [r"/select,C:\Users\Ы\file.txt", "--new-window"].iter().map(|s| s.as_bytes().to_vec()).collect();
    let mut open = parse_args(&args, Some(br"C:\cwd".to_vec()));
    open.paths.push(vec![0xED, 0xA0, 0x80]);
    assert_eq!(open.target, OpenTarget::NewWindow);
    let hello = Hello::new(Protocol::Instance, VERSION);
    let mut wire = Vec::new();
    write_frame(&mut wire, &hello).unwrap();
    write_frame(&mut wire, &Request::Open(open.clone())).unwrap();
    let mut r = Cursor::new(wire);
    let mut buf = Vec::new();
    let theirs: Hello = read_frame(&mut r, &mut buf).unwrap();
    hello.check(&theirs).unwrap();
    assert_eq!(read_frame::<_, Request>(&mut r, &mut buf).unwrap(), Request::Open(open));
}

#[test]
fn the_launcher_activation_token_travels_with_the_open() {
    let open = flit_core::Open { paths: vec![b"/tmp".to_vec()], activation: Some("kwin-42_TIME7".into()), ..Default::default() };
    let mut wire = Vec::new();
    write_frame(&mut wire, &Request::Open(open.clone())).unwrap();
    let back: Request = read_frame(&mut Cursor::new(wire), &mut Vec::new()).unwrap();
    assert_eq!(back, Request::Open(open));
    const { assert!(VERSION >= 3, "a new Open field needs a new protocol version") };
}
