use super::*;
use flit_core::archive::Format;
use flit_core::git::GitStatus;
use flit_core::FsPath;
use std::fs;
use std::ops::ControlFlow;
use std::path::Path;

type Target = (&'static str, fn(&[u8]));

const TARGETS: &[Target] = &[
    ("ntfs", ntfs),
    ("transport", transport),
    ("indexd", indexd),
    ("wsl_agent", wsl_agent),
    ("getdents", getdents),
    ("desktop_entry", desktop_entry),
    ("xbel", xbel),
    ("archive_headers", archive_headers),
    ("raw_tiff", raw_tiff),
    ("split_crc", split_crc),
    ("git_porcelain", git_porcelain),
    ("flit_link", flit_link),
];

#[test]
fn every_target_replays_its_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
    for (name, target) in TARGETS {
        let corpus = root.join(name);
        let seeds = fs::read_dir(&corpus).unwrap_or_else(|e| panic!("{}: {e}", corpus.display()));
        let mut count = 0;
        for seed in seeds {
            let path = seed.unwrap().path();
            let bytes = fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            target(&bytes);
            target(&bytes[..bytes.len().saturating_sub(1)]);
            count += 1;
        }
        assert!(count > 0, "{name} corpus is empty");
    }
}

#[test]
fn seeds_reach_the_parsers_past_their_headers() {
    let mut mft_bytes = *include_bytes!("../corpus/ntfs/file_record");
    assert!(mft::fixup(&mut mft_bytes));
    assert_eq!(mft::parse_record(&mft_bytes).unwrap().size, Some(4096));
    assert_eq!(mft::data_runs(&mft_bytes).and_then(mft::runs), Some(vec![(Some(32), 1)]));
    let usn = include_bytes!("../corpus/ntfs/journal");
    assert_eq!(mft::usn_records(usn).map(|r| r.frn).collect::<Vec<_>>(), [42, 43]);

    let mut hello = include_bytes!("../corpus/transport/hello").as_slice();
    let ours = transport::Hello::new(transport::Protocol::Instance, instance::VERSION);
    assert_eq!(transport::read_hello(&mut hello, &ours).unwrap().build, "seed");
    let mut query = include_bytes!("../corpus/indexd/query_done").as_slice();
    assert_eq!(transport::read_frame::<_, Message>(&mut query, &mut Vec::new()).unwrap(), Message::QueryDone { gen: 7, total: 8, version: 9 });
    let mut request = include_bytes!("../corpus/wsl_agent/shutdown").as_slice();
    assert_eq!(transport::read_frame::<_, provider::Request>(&mut request, &mut Vec::new()).unwrap(), provider::Request::Shutdown);
    let mut event = include_bytes!("../corpus/wsl_agent/list_done").as_slice();
    assert_eq!(transport::read_frame::<_, provider::Event>(&mut event, &mut Vec::new()).unwrap(), provider::Event::ListDone { id: 7, error: None });

    let mut entries = Vec::new();
    let read = dirent::for_each(include_bytes!("../corpus/getdents/file"), |name, kind, inode| {
        entries.push((name.to_bytes().to_vec(), kind, inode));
        true
    });
    assert_eq!(read, Some(ControlFlow::Continue(())));
    assert_eq!(entries, [(b"file".to_vec(), 8, 42)]);

    let entry = include_str!("../corpus/desktop_entry/localized");
    assert_eq!(desktop::name(entry, "sr_RS.UTF-8@latin", "fr_FR").as_deref(), Some("Latin App"));
    let bookmarks = include_bytes!("../corpus/xbel/bookmarks");
    assert_eq!(xbel_parser::bookmark_tags(bookmarks).filter_map(xbel_parser::bookmark_href).collect::<Vec<_>>(), [b"file:///home/test/a&b".to_vec()]);

    let zip = include_bytes!("../corpus/archive_headers/zip");
    assert_eq!(flit_archive::fuzz_archive_headers(zip).unwrap(), (Format::Zip, vec![b"note.txt".to_vec()]));
    let gz = include_bytes!("../corpus/archive_headers/gzip");
    assert_eq!(flit_archive::fuzz_archive_headers(gz).unwrap(), (Format::Gz, vec![b"note.txt".to_vec()]));
    let tar = include_bytes!("../corpus/archive_headers/tar");
    assert_eq!(flit_archive::fuzz_archive_headers(tar).unwrap().0, Format::Tar);

    let tiff = include_bytes!("../corpus/raw_tiff/preview");
    assert_eq!(tiff::locations(&mut Cursor::new(tiff), tiff.len() as u64, || true), Some((6, vec![(50, 4)])));
    let crc = include_bytes!("../corpus/split_crc/ansi");
    let manifest = segments::Manifest::parse(&segments::decode_manifest(crc, |_| None)).unwrap();
    assert_eq!((manifest.file.as_str(), manifest.len, manifest.checksum), ("café.txt", 9, 0xCBF4_3926));

    let status = include_bytes!("../corpus/git_porcelain/rename");
    assert_eq!(git::parse_tracking(status), (Some("origin/main".to_string()), 2, 1));
    let changed = git::parse_status(status, b"src/");
    assert_eq!(changed.get(b"new.txt".as_slice()), Some(&GitStatus::Renamed));
    assert_eq!(changed.get(b"notes.txt".as_slice()), Some(&GitStatus::Untracked));
    let uri = include_str!("../corpus/flit_link/folder");
    assert_eq!(link::parse(uri, false, &|p| p == Path::new("/home/Documents")), FsPath::parse("/home/Documents").map(link::Link::Folder));
}

#[test]
fn parsers_reject_truncated_or_invalid_input() {
    let mft_bytes = include_bytes!("../corpus/ntfs/file_record");
    assert!(!mft::fixup(&mut mft_bytes[..511].to_vec()));
    let dirent_bytes = include_bytes!("../corpus/getdents/file");
    assert_eq!(dirent::for_each(&dirent_bytes[..20], |_, _, _| true), None);

    let huge = u32::MAX.to_le_bytes();
    let mut packet = huge.as_slice();
    assert!(matches!(transport::read_frame::<_, Message>(&mut packet, &mut Vec::new()), Err(transport::ProtoError::TooLarge(_))));
    assert!(flit_archive::fuzz_archive_headers(b"PK").is_err());
    assert!(tiff::locations(&mut Cursor::new(b"II"), 2, || true).is_none());
    assert!(segments::Manifest::parse("filename=report.txt\nsize=not-a-number\ncrc32=CBF43926").is_none());
    assert!(link::parse("flit:open?path=/home/%FF", false, &|_| true).is_none());
}
