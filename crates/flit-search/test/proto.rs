use super::*;
use flit_core::Meta;
use flit_proto::transport::{decode, encode, read_frame, write_frame, ProtoError};
use std::io::Cursor;

fn all_messages() -> Vec<Message> {
    let sort = Sort { key: SortKey::Size, descending: true };
    let mut entries = EntryBatch::default();
    entries.push(b"a.txt", Meta { size: 5, modified: 133_000_000_000_000_000, ..Default::default() });
    entries.push(&[0xED, 0xA0, 0x80, b'x'], Meta { size: u64::MAX, modified: -1, created: i64::MIN, attrs: u32::MAX });
    vec![
        Message::Query { gen: 7, text: "ext:rs отчёт 🎉".into(), sort },
        Message::QueryDone { gen: 7, total: 1_234_567 },
        Message::GetRows { gen: 7, from: 100, to: 160 },
        Message::Rows { gen: 7, from: 100, entries, dirs: vec![br"C:\".to_vec(), b"/tmp/\xFF\xFE".to_vec()] },
        Message::IndexStatus {
            volumes: vec![
                VolumeInfo { id: "C:".into(), label: "System".into(), file_system: "NTFS".into(), files: 2_000_000, state: VolumeState::Scanning { progress: 0.5 } },
                VolumeInfo { id: "/".into(), label: String::new(), file_system: "ext4".into(), files: 0, state: VolumeState::Error("permission denied".into()) },
            ],
        },
        Message::Error { message: "bad query".into() },
    ]
}

#[test]
fn roundtrip_every_message() {
    for m in all_messages() {
        let mut buf = Vec::new();
        encode(&m, &mut buf).unwrap();
        let len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
        assert_eq!(len + 4, buf.len());
        assert_eq!(decode::<Message>(&buf[4..]).unwrap(), m);
    }
}

#[test]
fn stream_of_frames() {
    let mut wire = Vec::new();
    for m in all_messages() {
        write_frame(&mut wire, &m).unwrap();
    }
    let mut r = Cursor::new(wire);
    let mut buf = Vec::new();
    for m in all_messages() {
        assert_eq!(read_frame::<_, Message>(&mut r, &mut buf).unwrap(), m);
    }
    assert!(matches!(read_frame::<_, Message>(&mut r, &mut buf), Err(ProtoError::Io(_))));
}

#[test]
fn compact_encoding() {
    let mut buf = Vec::new();
    encode(&Message::GetRows { gen: 1, from: 0, to: 60 }, &mut buf).unwrap();
    assert!(buf.len() <= 9, "varints keep small messages tiny: {} bytes", buf.len());
}
