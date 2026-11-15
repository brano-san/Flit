use super::*;
use flit_platform::CANCELLED;
use std::io::{self, Cursor, Write};

const FORMATS: [(Format, &str); 3] = [(Format::SevenZ, "7z"), (Format::Tar, "tar"), (Format::TarGz, "tar.gz")];

fn add(t: &Temp, archive: &FsPath, names: &[&str], policy: Resolve) -> OpOutcome {
    run(FileOp::Copy { items: names.iter().map(|n| t.fs(n)).collect(), dest: archive.clone() }, policy)
}

fn no_parts(t: &Temp) {
    assert!(!fs::read_dir(&t.0).unwrap().any(|e| { e.unwrap().file_name().to_string_lossy().ends_with(".flit-part") }));
}

#[test]
fn adding_to_non_zip_preserves_files_empty_folders_and_counts_old_and_new_bytes() {
    for (format, ext) in FORMATS {
        let t = Temp::new(ext);
        let archive = t.fs(&format!("a.{ext}"));
        compress(sample(&t), archive.clone(), format, None);
        t.write("new/folder/note.txt", b"new content");
        fs::create_dir_all(t.path("new/folder/empty")).unwrap();
        t.write("new/one.txt", b"one");
        let job = Job::detached(Resolve::Replace);
        let out = file_op(&FileOp::Copy { items: vec![t.fs("new/folder"), t.fs("new/one.txt")], dest: archive.clone() }, &job, &env()).unwrap();
        assert!(out.error.is_none(), "{ext}: {:?}", out.error);
        let progress = job.progress();
        assert_eq!(progress.total_bytes, 200_000 + "квартальный отчёт".len() as u64 + 13 + 11 + 3, "{ext}");
        assert_eq!(progress.bytes, progress.total_bytes, "{ext}");
        assert_eq!(progress.items, progress.total_items, "{ext}");
        let out = extract(&archive, &t.fs("out"), Place::Here);
        assert!(out.error.is_none(), "{ext}: {:?}", out.error);
        assert_eq!(t.read("out/readme.md"), b"hello archive");
        assert_eq!(t.read("out/Документы/deep/data.bin"), t.read("src/Документы/deep/data.bin"));
        assert_eq!(t.read("out/folder/note.txt"), b"new content");
        assert_eq!(t.read("out/one.txt"), b"one");
        assert!(t.path("out/empty").is_dir() && t.path("out/folder/empty").is_dir());
        no_parts(&t);
    }
}

#[test]
fn adding_to_non_zip_uses_zip_conflict_rules_for_whole_folders() {
    for (format, ext) in FORMATS {
        for policy in [Resolve::Replace, Resolve::KeepBoth, Resolve::Skip, Resolve::ReplaceOlder] {
            let t = Temp::new(ext);
            t.write("old/Folder/old.txt", b"old");
            t.write("old/keep.txt", b"keep");
            let archive = t.fs(&format!("a.{ext}"));
            compress(vec![t.fs("old/Folder"), t.fs("old/keep.txt")], archive.clone(), format, None);
            t.write("new/folder/new.txt", b"new");
            let out = add(&t, &archive, &["new/folder"], policy);
            assert!(out.error.is_none(), "{ext}: {:?}", out.error);
            assert!(extract(&archive, &t.fs("out"), Place::Here).error.is_none());
            assert_eq!(t.read("out/keep.txt"), b"keep");
            match policy {
                Resolve::Replace => {
                    assert!(!t.path("out/Folder/old.txt").exists());
                    assert_eq!(t.read("out/folder/new.txt"), b"new");
                }
                Resolve::KeepBoth => {
                    assert_eq!(t.read("out/Folder/old.txt"), b"old");
                    assert_eq!(t.read("out/folder (2)/new.txt"), b"new");
                }
                _ => {
                    assert_eq!(t.read("out/Folder/old.txt"), b"old");
                    assert!(!t.path("out/folder/new.txt").exists());
                }
            }
            no_parts(&t);
        }
    }
}

#[test]
fn cancelling_a_non_zip_rewrite_midway_leaves_original_bytes_and_no_temporary_file() {
    for (format, ext) in FORMATS {
        let t = Temp::new(ext);
        let bytes: Vec<u8> = (0..8 * 1024 * 1024u32).map(|i| i.wrapping_mul(1664525).wrapping_add(i >> 7) as u8).collect();
        t.write("old.bin", &bytes);
        let archive = t.fs(&format!("a.{ext}"));
        compress(vec![t.fs("old.bin")], archive.clone(), format, None);
        let before = fs::read(std_path(&archive)).unwrap();
        t.write("new.bin", &bytes);
        let job = Job::detached(Resolve::Replace);
        let op = FileOp::Copy { items: vec![t.fs("new.bin")], dest: archive.clone() };
        let out = std::thread::scope(|scope| {
            let task = scope.spawn(|| file_op(&op, &job, &env()).unwrap());
            while job.progress().bytes < 64 * 1024 && !task.is_finished() {
                std::thread::yield_now();
            }
            let p = job.progress();
            assert!(p.bytes >= 64 * 1024 && p.bytes < p.total_bytes, "{ext}: {p:?}");
            job.ctl().cancel();
            task.join().unwrap()
        });
        assert_eq!(out.error.unwrap().message, CANCELLED, "{ext}");
        assert_eq!(fs::read(std_path(&archive)).unwrap(), before, "{ext}");
        no_parts(&t);
    }
}

#[test]
fn read_only_non_zip_archives_are_not_replaced_even_when_the_directory_is_writable() {
    for (format, ext) in FORMATS {
        let t = Temp::new(ext);
        t.write("old.txt", b"old");
        t.write("new.txt", b"new");
        let archive = t.fs(&format!("a.{ext}"));
        compress(vec![t.fs("old.txt")], archive.clone(), format, None);
        let path = std_path(&archive);
        let before = fs::read(&path).unwrap();
        let permissions = fs::metadata(&path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        let out = add(&t, &archive, &["new.txt"], Resolve::Replace);
        fs::set_permissions(&path, permissions).unwrap();
        let error = out.error.expect("read-only archives must refuse modification");
        assert_eq!(std::io::Error::from_raw_os_error(error.code as i32).kind(), std::io::ErrorKind::PermissionDenied, "{error:?}");
        assert_eq!(fs::read(&path).unwrap(), before);
        no_parts(&t);
    }
}

#[test]
fn sevenz_rewrite_preserves_all_available_entry_metadata() {
    let t = Temp::new("7z-metadata");
    let mut writer = sevenz_rust2::ArchiveWriter::new(Cursor::new(Vec::new())).unwrap();
    for (name, dir) in [("old.txt", false), ("empty", true)] {
        let mut entry = sevenz_rust2::ArchiveEntry::new_file(name);
        entry.is_directory = dir;
        entry.has_creation_date = true;
        entry.creation_date = 132_456_789_000_000_000u64.into();
        entry.has_access_date = true;
        entry.access_date = 132_456_790_000_000_000u64.into();
        entry.has_last_modified_date = true;
        entry.last_modified_date = 132_456_791_000_000_000u64.into();
        entry.has_windows_attributes = true;
        entry.windows_attributes = if dir { 0x12 } else { 0x23 };
        writer.push_archive_entry(entry, (!dir).then_some(b"old".as_slice())).unwrap();
    }
    t.write("a.7z", &writer.finish().unwrap().into_inner());
    let archive = t.fs("a.7z");
    let before = index::open(&archive, 0).unwrap().sevenz.as_ref().unwrap().files.clone();
    t.write("new.txt", b"new");
    assert!(add(&t, &archive, &["new.txt"], Resolve::Replace).error.is_none());
    let after = index::open(&archive, 0).unwrap();
    for old in before {
        let entry = after.sevenz.as_ref().unwrap().files.iter().find(|e| e.name == old.name).unwrap();
        assert_eq!((entry.has_creation_date, entry.creation_date, entry.has_access_date, entry.access_date), (old.has_creation_date, old.creation_date, old.has_access_date, old.access_date));
        assert_eq!(
            (entry.has_last_modified_date, entry.last_modified_date, entry.has_windows_attributes, entry.windows_attributes, entry.is_directory, entry.is_anti_item),
            (old.has_last_modified_date, old.last_modified_date, old.has_windows_attributes, old.windows_attributes, old.is_directory, old.is_anti_item)
        );
    }
}

#[test]
fn tar_rewrite_preserves_raw_headers_and_extended_metadata() {
    for (format, ext) in [(Format::Tar, "tar"), (Format::TarGz, "tgz")] {
        let t = Temp::new(ext);
        let mut writer = tar::Builder::new(Vec::new());
        writer.append_pax_extensions([("mtime", b"1234567890.123456789".as_slice()), ("SCHILY.xattr.user.test", b"metadata".as_slice())]).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_uid(123);
        header.set_gid(456);
        header.set_mode(0o751);
        header.set_mtime(1234567890);
        header.set_username("owner").unwrap();
        header.set_groupname("group").unwrap();
        header.set_size(3);
        writer.append_data(&mut header, "old.txt", b"old".as_slice()).unwrap();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        writer.append_data(&mut header, "empty", io::empty()).unwrap();
        writer.append_data(&mut header, ".", io::empty()).unwrap();
        let before = writer.into_inner().unwrap();
        let archive = t.fs(&format!("a.{ext}"));
        if format == Format::TarGz {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            gz.write_all(&before).unwrap();
            fs::write(std_path(&archive), gz.finish().unwrap()).unwrap();
        } else {
            fs::write(std_path(&archive), &before).unwrap();
        }
        t.write("new.txt", b"new");
        let job = Job::detached(Resolve::Replace);
        let out = file_op(&FileOp::Copy { items: vec![t.fs("new.txt")], dest: archive.clone() }, &job, &env()).unwrap();
        assert!(out.error.is_none(), "{:?}", out.error);
        assert_eq!(job.progress().items, job.progress().total_items);
        assert_eq!(job.progress().bytes, job.progress().total_bytes);
        let mut after = Vec::new();
        if format == Format::TarGz {
            flate2::read::MultiGzDecoder::new(fs::File::open(std_path(&archive)).unwrap()).read_to_end(&mut after).unwrap();
        } else {
            after = fs::read(std_path(&archive)).unwrap();
        }
        assert_eq!(&after[..before.len() - 1024], &before[..before.len() - 1024], "old raw entries, PAX attributes and padding must be preserved");
    }
}

#[test]
fn encrypted_sevenz_with_only_empty_entries_is_refused_with_a_cached_password() {
    let t = Temp::new("7z-encrypted-empty");
    t.write("empty.txt", b"");
    t.write("new.txt", b"new");
    let archive = t.fs("a.7z");
    compress(vec![t.fs("empty.txt")], archive.clone(), Format::SevenZ, Some("secret"));
    archive::set_password(&archive, Some(Secret("secret".into())));
    let before = fs::read(std_path(&archive)).unwrap();
    let error = add(&t, &archive, &["new.txt"], Resolve::Replace).error.unwrap();
    assert_eq!(error.message, flit_core::i18n::S::ErrorsArchiveEncryptedAdd.en());
    assert_eq!(fs::read(std_path(&archive)).unwrap(), before);
    no_parts(&t);
}

#[test]
fn tar_pax_size_overrides_survive_rewriting() {
    for (format, ext) in [(Format::Tar, "tar"), (Format::TarGz, "tgz")] {
        let t = Temp::new(ext);
        let mut writer = tar::Builder::new(Vec::new());
        writer.append_pax_extensions([("size", b"4".as_slice())]).unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_mode(0o644);
        header.set_size(3);
        header.set_mtime(0);
        writer.append_data(&mut header, "old.txt", b"data".as_slice()).unwrap();
        let before = writer.into_inner().unwrap();
        let archive = t.fs(&format!("a.{ext}"));
        if format == Format::TarGz {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            gz.write_all(&before).unwrap();
            fs::write(std_path(&archive), gz.finish().unwrap()).unwrap();
        } else {
            fs::write(std_path(&archive), before).unwrap();
        }
        t.write("new.txt", b"new");
        assert!(add(&t, &archive, &["new.txt"], Resolve::Replace).error.is_none());
        assert!(extract(&archive, &t.fs("out"), Place::Here).error.is_none());
        assert_eq!(t.read("out/old.txt"), b"data");
        assert_eq!(t.read("out/new.txt"), b"new");
    }
}

#[test]
fn tar_extended_sparse_members_keep_their_data_when_last_or_before_a_replaced_member() {
    for (format, ext) in [(Format::Tar, "tar"), (Format::TarGz, "tgz")] {
        for trailing in [false, true] {
            let t = Temp::new("sparse-rewrite");
            let mut header = tar::Header::new_gnu();
            header.set_path("sparse.bin").unwrap();
            header.set_entry_type(tar::EntryType::GNUSparse);
            header.set_mode(0o600);
            header.set_mtime(0);
            header.set_size(5 * 512);
            let gnu = header.as_gnu_mut().unwrap();
            gnu.set_real_size(9 * 512);
            gnu.set_is_extended(true);
            for (i, extent) in gnu.sparse.iter_mut().enumerate() {
                extent.set_offset(i as u64 * 1024);
                extent.set_length(512);
            }
            header.set_cksum();
            let mut extension = tar::GnuExtSparseHeader::new();
            extension.sparse[0].set_offset(4096);
            extension.sparse[0].set_length(512);
            let mut bytes = header.as_bytes().to_vec();
            bytes.extend_from_slice(extension.as_bytes());
            let mut expected = vec![0; 9 * 512];
            for i in 0..5 {
                bytes.extend_from_slice(&vec![b'A' + i as u8; 512]);
                expected[i * 1024..i * 1024 + 512].fill(b'A' + i as u8);
            }
            if trailing {
                let mut writer = tar::Builder::new(Vec::new());
                let mut header = tar::Header::new_gnu();
                header.set_size(3);
                header.set_mode(0o600);
                header.set_mtime(0);
                writer.append_data(&mut header, "new.txt", b"old".as_slice()).unwrap();
                bytes.extend_from_slice(&writer.into_inner().unwrap());
            } else {
                bytes.extend_from_slice(&[0; 1024]);
            }
            let archive = t.fs(&format!("a.{ext}"));
            if format == Format::TarGz {
                let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                gzip.write_all(&bytes).unwrap();
                fs::write(std_path(&archive), gzip.finish().unwrap()).unwrap();
            } else {
                fs::write(std_path(&archive), bytes).unwrap();
            }
            t.write("new.txt", b"new");
            let out = add(&t, &archive, &["new.txt"], Resolve::Replace);
            assert!(out.error.is_none(), "{ext}: {:?}", out.error);
            let reader: Box<dyn Read> = if format == Format::TarGz {
                Box::new(flate2::read::MultiGzDecoder::new(fs::File::open(std_path(&archive)).unwrap()))
            } else {
                Box::new(fs::File::open(std_path(&archive)).unwrap())
            };
            let mut archive = tar::Archive::new(reader);
            let mut found = Vec::new();
            for entry in archive.entries().unwrap() {
                let mut entry = entry.unwrap();
                let name = entry.path_bytes().into_owned();
                let mut contents = Vec::new();
                entry.read_to_end(&mut contents).unwrap();
                found.push((name, contents));
            }
            assert_eq!(found, vec![(b"sparse.bin".to_vec(), expected), (b"new.txt".to_vec(), b"new".to_vec())], "{ext}, trailing={trailing}");
        }
    }
}

#[cfg(unix)]
#[test]
fn private_archives_and_their_in_progress_temporary_files_remain_private() {
    use flit_core::provider::OpCtl;
    use std::os::unix::fs::PermissionsExt;
    for (format, ext) in FORMATS {
        let t = Temp::new("private-rewrite");
        t.write("old.txt", &vec![b'x'; 1024 * 1024]);
        t.write("new.txt", b"new");
        let archive = t.fs(&format!("a.{ext}"));
        compress(vec![t.fs("old.txt")], archive.clone(), format, None);
        let path = std_path(&archive);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let before = fs::read(&path).unwrap();
        let ctl = OpCtl::new(Some(Resolve::Replace));
        let pause = ctl.clone();
        let job = Job::new(ctl, Box::new(move |_| pause.pause(true)), Box::new(|_| {}));
        let op = FileOp::Copy { items: vec![t.fs("new.txt")], dest: archive.clone() };
        let (mode, out) = std::thread::scope(|scope| {
            let task = scope.spawn(|| file_op(&op, &job, &env()).unwrap());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mode = loop {
                let part = fs::read_dir(&t.0).unwrap().filter_map(Result::ok).find(|e| e.file_name().to_string_lossy().ends_with(".flit-part"));
                if let Some(part) = part {
                    break Some(part.metadata().unwrap().permissions().mode() & 0o777);
                }
                if task.is_finished() || std::time::Instant::now() >= deadline {
                    break None;
                }
                std::thread::yield_now();
            };
            job.ctl().cancel();
            job.ctl().pause(false);
            (mode, task.join().unwrap())
        });
        assert_eq!(mode, Some(0o600), "{ext}: temporary archive must start private");
        assert_eq!(out.error.unwrap().message, CANCELLED);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        no_parts(&t);
        assert!(add(&t, &archive, &["new.txt"], Resolve::Replace).error.is_none());
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "{ext}: final archive remains private");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn rewritten_archives_preserve_owner_group_and_source_acl_without_inherited_grants() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    fn acl(user: u32, owner: u16, mask: u16) -> Vec<u8> {
        let mut bytes = 2u32.to_le_bytes().to_vec();
        for (tag, permissions, id) in [(1u16, owner, u32::MAX), (2, 4, user), (4, 0, u32::MAX), (16, mask, u32::MAX), (32, 0, u32::MAX)] {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&permissions.to_le_bytes());
            bytes.extend_from_slice(&id.to_le_bytes());
        }
        bytes
    }
    fn read_acl(file: &fs::File) -> Option<Vec<u8>> {
        let mut bytes = vec![0; 65536];
        let size = unsafe { libc::fgetxattr(file.as_raw_fd(), c"system.posix_acl_access".as_ptr(), bytes.as_mut_ptr().cast(), bytes.len()) };
        if size < 0 {
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ENODATA));
            None
        } else {
            bytes.truncate(size as usize);
            Some(bytes)
        }
    }
    for (format, ext) in FORMATS {
        for source_acl in [false, true] {
            let t = Temp::new("acl-rewrite");
            t.write("old.txt", b"private");
            t.write("new.txt", b"new");
            let archive = t.fs(&format!("a.{ext}"));
            compress(vec![t.fs("old.txt")], archive.clone(), format, None);
            let path = std_path(&archive);
            let original = fs::File::open(&path).unwrap();
            if unsafe { libc::geteuid() } == 0 {
                assert_eq!(unsafe { libc::fchown(original.as_raw_fd(), u32::MAX, 65534) }, 0);
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let directory = fs::File::open(&t.0).unwrap();
            let inherited = acl(65533, 7, 6);
            if unsafe { libc::fsetxattr(directory.as_raw_fd(), c"system.posix_acl_default".as_ptr(), inherited.as_ptr().cast(), inherited.len(), 0) } != 0 {
                let error = io::Error::last_os_error();
                assert_eq!(error.raw_os_error(), Some(libc::ENOTSUP), "{error}");
                return;
            }
            let expected_acl = if source_acl {
                let bytes = acl(65534, 6, 4);
                assert_eq!(unsafe { libc::fsetxattr(original.as_raw_fd(), c"system.posix_acl_access".as_ptr(), bytes.as_ptr().cast(), bytes.len(), 0) }, 0);
                Some(bytes)
            } else {
                None
            };
            let expected = original.metadata().unwrap();
            let out = add(&t, &archive, &["new.txt"], Resolve::Replace);
            assert!(out.error.is_none(), "{ext}: {:?}", out.error);
            let rewritten = fs::File::open(&path).unwrap();
            let actual = rewritten.metadata().unwrap();
            assert_eq!((actual.mode() & 0o7777, actual.uid(), actual.gid()), (expected.mode() & 0o7777, expected.uid(), expected.gid()), "{ext}");
            assert_eq!(read_acl(&rewritten), expected_acl, "{ext}: inherited grants must not survive");
            assert_eq!(t.read("old.txt"), b"private");
            no_parts(&t);
        }
    }
}

#[test]
fn encrypted_sevenz_refuses_addition_with_or_without_a_cached_password() {
    for encrypt_names in [false, true] {
        let t = Temp::new("7z-encrypted-add");
        t.write("old.txt", b"old");
        t.write("new.txt", b"new");
        let archive = t.fs("a.7z");
        let options = Compression { format: Format::SevenZ, password: Some(Secret("secret".into())), encrypt_names, ..Compression::default() };
        assert!(run(FileOp::Archive(ArchiveOp::Compress { items: vec![t.fs("old.txt")], archive: archive.clone(), options }), Resolve::Replace).error.is_none());
        let before = fs::read(std_path(&archive)).unwrap();
        for password in [None, Some(Secret("secret".into()))] {
            archive::set_password(&archive, password);
            let error = add(&t, &archive, &["new.txt"], Resolve::Replace).error.unwrap();
            assert!(error.message.contains("adding files to encrypted 7z archives is not supported"), "{error:?}");
            assert_eq!(fs::read(std_path(&archive)).unwrap(), before);
            no_parts(&t);
        }
    }
}

#[test]
fn rar_lzh_and_single_gzip_refuse_addition_without_modifying_bytes() {
    for name in ["a.rar", "a.lzh", "a.gz"] {
        let t = Temp::new("unsupported-add");
        t.write("new.txt", b"new");
        let bytes: &[u8] = if name.ends_with("rar") {
            b"Rar!\x1a\x07\x00"
        } else if name.ends_with("lzh") {
            b"\x00\x00-lh0-"
        } else {
            b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x00"
        };
        t.write(name, bytes);
        let error = add(&t, &t.fs(name), &["new.txt"], Resolve::Replace).error.unwrap();
        assert_eq!(error.message, flit_core::i18n::S::ErrorsArchiveNoAdd.en());
        assert_eq!(t.read(name), bytes);
        no_parts(&t);
    }
}

#[cfg(windows)]
#[test]
fn rewritten_archives_keep_the_windows_internet_zone_and_propagate_it_when_extracted() {
    for (format, ext) in FORMATS {
        let t = Temp::new("rewrite-zone");
        t.write("old.txt", b"old");
        t.write("new.txt", b"new");
        let archive = t.fs(&format!("a.{ext}"));
        compress(vec![t.fs("old.txt")], archive.clone(), format, None);
        let mark = b"[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.org/download\r\n";
        zone::mark(&std_path(&archive), mark);
        assert_eq!(zone::read(&std_path(&archive)).as_deref(), Some(mark.as_slice()));
        assert!(add(&t, &archive, &["new.txt"], Resolve::Replace).error.is_none());
        assert_eq!(zone::read(&std_path(&archive)).as_deref(), Some(mark.as_slice()), "{ext}");
        assert!(extract(&archive, &t.fs("out"), Place::Here).error.is_none());
        assert_eq!(zone::read(&t.path("out/old.txt")).as_deref(), Some(mark.as_slice()));
        assert_eq!(zone::read(&t.path("out/new.txt")).as_deref(), Some(mark.as_slice()));
    }
}
