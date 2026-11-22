#[path = "../../crates/flit-platform-linux/src/desktop_entry.rs"]
mod desktop;
#[path = "../../crates/flit-platform-linux/src/dirent.rs"]
mod dirent;
#[path = "../../crates/flit/src/link_uri.rs"]
mod link;
#[path = "../../crates/flit-platform-windows/src/mft_record.rs"]
mod mft;
#[path = "../../crates/flit/src/preview/raw_tiff.rs"]
mod tiff;
#[path = "../../crates/flit-platform/src/segment_manifest.rs"]
mod segments;
#[path = "../../crates/flit-platform-linux-gui/src/xbel.rs"]
mod xbel_parser;

use flit_core::git;
use flit_proto::{instance, listing::EntryBatch, provider, transport};
use flit_search::proto::{Message, VERSION as INDEX_VERSION};
use std::hint::black_box;
use std::io::Cursor;

const MAX_FRAME: usize = 64 << 10;

pub fn ntfs(bytes: &[u8]) {
    for record in mft::usn_records(bytes) {
        black_box((record.frn, record.parent, record.usn, record.time, record.reason, record.attrs, record.name));
    }
    let mut record = bytes.to_vec();
    if mft::fixup(&mut record) {
        black_box(mft::parse_record(&record));
        black_box(mft::data_runs(&record).and_then(mft::runs));
    }
}

pub fn transport(bytes: &[u8]) {
    let hello = transport::Hello::new(transport::Protocol::Instance, instance::VERSION);
    let _ = transport::read_hello(&mut Cursor::new(bytes), &hello);
    let mut buffer = Vec::new();
    if let Ok(entries) = transport::read_frame_limited::<_, EntryBatch>(&mut Cursor::new(bytes), &mut buffer, MAX_FRAME) {
        let _ = entries.entries(true).map(|entries| entries.count());
        let _ = entries.entries(false).map(|entries| entries.count());
    }
    let _ = transport::read_frame_limited::<_, instance::Request>(&mut Cursor::new(bytes), &mut buffer, MAX_FRAME);
    let _ = transport::read_frame_limited::<_, instance::Reply>(&mut Cursor::new(bytes), &mut buffer, MAX_FRAME);
}

pub fn indexd(bytes: &[u8]) {
    let hello = transport::Hello::new(transport::Protocol::Index, INDEX_VERSION);
    let _ = transport::read_hello(&mut Cursor::new(bytes), &hello);
    let mut buffer = Vec::new();
    if let Ok(Message::Rows { entries, .. }) = transport::read_frame_limited::<_, Message>(&mut Cursor::new(bytes), &mut buffer, MAX_FRAME) {
        let _ = entries.entries(true).map(|entries| entries.count());
        let _ = entries.entries(false).map(|entries| entries.count());
    }
}

pub fn wsl_agent(bytes: &[u8]) {
    let hello = transport::Hello::new(transport::Protocol::Provider, provider::VERSION);
    let _ = transport::read_hello(&mut Cursor::new(bytes), &hello);
    let mut buffer = Vec::new();
    if let Ok(provider::Request::Replay { plan, .. }) = transport::read_frame_limited::<_, provider::Request>(&mut Cursor::new(bytes), &mut buffer, MAX_FRAME) {
        let _ = provider::check_replay(&plan);
    }
    if let Ok(provider::Event::Entries { batch, .. }) = transport::read_frame_limited::<_, provider::Event>(&mut Cursor::new(bytes), &mut buffer, MAX_FRAME) {
        let _ = batch.entries(false).map(|entries| entries.count());
    }
}

pub fn getdents(bytes: &[u8]) {
    let _ = dirent::for_each(bytes, |name, kind, inode| {
        black_box((name, kind, inode));
        true
    });
}

pub fn desktop_entry(bytes: &[u8]) {
    if let Ok(text) = std::str::from_utf8(bytes) {
        black_box(desktop::name(text, "sr_RS.UTF-8@latin", "fr_FR"));
    }
}

pub fn xbel(bytes: &[u8]) {
    for tag in xbel_parser::bookmark_tags(bytes) {
        if let Some(href) = xbel_parser::bookmark_href(tag) {
            black_box(href);
            black_box(xbel_parser::attribute(tag, b"modified").or_else(|| xbel_parser::attribute(tag, b"visited")));
        }
    }
}

pub fn archive_headers(bytes: &[u8]) {
    let _ = black_box(flit_archive::fuzz_archive_headers(bytes));
}

pub fn raw_tiff(bytes: &[u8]) {
    if let Some((orientation, images)) = tiff::locations(&mut Cursor::new(bytes), bytes.len() as u64, || true) {
        assert!((1..=8).contains(&orientation));
        for (offset, len) in images {
            assert!(u64::from(offset) + u64::from(len) <= bytes.len() as u64);
        }
    }
}

pub fn split_crc(bytes: &[u8]) {
    if let Some(manifest) = segments::Manifest::parse(&segments::decode_manifest(bytes, |_| None)) {
        let round_trip = segments::Manifest::parse(&manifest.render()).map(|parsed| (parsed.file, parsed.len, parsed.checksum));
        assert_eq!(round_trip, Some((manifest.file, manifest.len, manifest.checksum)));
    }
}

pub fn git_porcelain(bytes: &[u8]) {
    black_box(git::parse_tracking(bytes));
    black_box(git::parse_status(bytes, b""));
    black_box(git::parse_status(bytes, b"src/"));
}

pub fn flit_link(bytes: &[u8]) {
    if let Ok(uri) = std::str::from_utf8(bytes) {
        black_box(link::parse(uri, true, &|_| true));
        black_box(link::parse(uri, false, &|_| true));
    }
}

#[cfg(test)]
#[path = "../test/corpus.rs"]
mod tests;
