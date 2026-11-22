use super::*;
use flit_core::provider::{FileOp, Resolve};

struct Folder(std::path::PathBuf);

impl Folder {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!("flit-segments-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let dir = std::env::temp_dir().join(name);
        fs::create_dir(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        Self(dir)
    }

    fn path(&self) -> FsPath {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            FsPath::from_utf16(&self.0.as_os_str().encode_wide().collect::<Vec<_>>()).unwrap()
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            FsPath::parse_exact(self.0.as_os_str().as_bytes()).unwrap()
        }
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Publisher;

impl FileOps for Publisher {
    fn rename_local_noreplace(&mut self, from: &FsPath, to: &FsPath) -> Result<(), PlatformError> {
        assert!(!std_path(to).exists());
        fs::rename(std_path(from), std_path(to)).unwrap();
        Ok(())
    }

    fn file_op(&mut self, _: &FileOp, _: &Job) -> OpOutcome {
        unreachable!()
    }

    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        unreachable!()
    }
    fn clear_clipboard(&mut self) {
        unreachable!()
    }
}

#[test]
fn split_and_join_round_trip_total_commander_names_and_check_progress() {
    let temp = Folder::new();
    let dir = temp.path();
    let source = dir.join(b"letter.txt");
    fs::write(std_path(&source), b"123456789").unwrap();
    let split_job = Job::detached(Resolve::Skip);
    let split = run(&mut Publisher, &source, &dir, Some(4), &split_job);
    assert!(split.error.is_none(), "{:?}", split.error);
    assert_eq!(split.items.len(), 4);
    assert_eq!(split_job.progress().bytes, 9);
    assert_eq!(fs::read(temp.0.join("letter.001")).unwrap(), b"1234");
    assert_eq!(fs::read(temp.0.join("letter.003")).unwrap(), b"9");
    assert_eq!(fs::read(temp.0.join("letter.crc")).unwrap(), b"filename=letter.txt\r\nsize=9\r\ncrc32=CBF43926\r\n");
    let output = dir.join(b"output");
    fs::create_dir(std_path(&output)).unwrap();
    let joined = run(&mut Publisher, &dir.join(b"letter.002"), &output, None, &Job::detached(Resolve::Skip));
    assert!(joined.error.is_none(), "{:?}", joined.error);
    assert_eq!(fs::read(std_path(&output.join(b"letter.txt"))).unwrap(), b"123456789");
}

#[test]
fn invalid_manifest_corrupt_content_and_cancelled_jobs_leave_no_output() {
    let temp = Folder::new();
    let dir = temp.path();
    let from = dir.join(b"report.pdf.crc");
    let part = dir.join(b"report.pdf.001");
    fs::write(std_path(&part), b"a").unwrap();
    let output = dir.join(b"output");
    fs::create_dir(std_path(&output)).unwrap();
    for manifest in ["filename=outside.pdf\r\nsize=1\r\ncrc32=00000000\r\n", "filename=../escaped.pdf\r\nsize=1\r\ncrc32=E8B7BE43\r\n"] {
        fs::write(std_path(&from), manifest).unwrap();
        let bad = run(&mut Publisher, &from, &output, None, &Job::detached(Resolve::Skip));
        assert!(bad.error.is_some() && bad.items.is_empty());
        assert_eq!(fs::read_dir(std_path(&output)).unwrap().count(), 0);
    }
    fs::write(std_path(&from), "\u{feff}filename=отчёт.pdf\r\nsize=1\r\ncrc32=E8B7BE43\r\n").unwrap();
    let job = Job::detached(Resolve::Skip);
    job.ctl().cancel();
    assert_eq!(run(&mut Publisher, &part, &output, None, &job).error.map(|e| e.kind), Some(crate::PlatformErrorKind::Cancelled));
    assert_eq!(fs::read_dir(std_path(&output)).unwrap().count(), 0);
    let good = run(&mut Publisher, &from, &output, None, &Job::detached(Resolve::Skip));
    assert!(good.error.is_none(), "{:?}", good.error);
    assert_eq!(fs::read(temp.0.join("output").join("отчёт.pdf")).unwrap(), b"a");
}

#[test]
fn joins_numbered_parts_without_a_crc_file() {
    let temp = Folder::new();
    let dir = temp.path();
    fs::write(temp.0.join("movie.mp4.001"), b"one").unwrap();
    fs::write(temp.0.join("movie.mp4.002"), b"two").unwrap();
    let job = Job::detached(Resolve::Skip);
    let joined = run(&mut Publisher, &dir.join(b"movie.mp4.002"), &dir, None, &job);
    assert!(matches!(job.take_notice(), Some(S::SegmentsNoCrc)));
    assert!(joined.error.is_none(), "{:?}", joined.error);
    assert_eq!(fs::read(temp.0.join("movie.mp4")).unwrap(), b"onetwo");
}

#[test]
fn joins_parts_with_a_windows_1252_manifest() {
    let temp = Folder::new();
    let dir = temp.path();
    fs::write(temp.0.join("set.001"), b"a").unwrap();
    fs::write(temp.0.join("set.crc"), b"filename=caf\xe9.txt\r\nsize=1\r\ncrc32=E8B7BE43\r\n").unwrap();
    let job = Job::detached(Resolve::Skip);
    let joined = run(&mut Publisher, &dir.join(b"set.001"), &dir, None, &job);
    assert!(joined.error.is_none(), "{:?}", joined.error);
    assert!(job.take_notice().is_none());
    assert_eq!(fs::read(temp.0.join("café.txt")).unwrap(), b"a");
}

struct Interrupted<'a> {
    job: Option<&'a Job>,
    first: bool,
}

impl FileOps for Interrupted<'_> {
    fn rename_local_noreplace(&mut self, from: &FsPath, to: &FsPath) -> Result<(), PlatformError> {
        if std_path(to).exists() {
            return Err(PlatformError::new(-1, "collision"));
        }
        Publisher.rename_local_noreplace(from, to)?;
        if std::mem::replace(&mut self.first, false) {
            if let Some(job) = self.job {
                job.ctl().cancel();
            } else {
                fs::write(std_path(&to.parent().unwrap().join(b"letter.002")), b"someone else's file").unwrap();
            }
        }
        Ok(())
    }

    fn file_op(&mut self, _: &FileOp, _: &Job) -> OpOutcome {
        unreachable!()
    }
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        unreachable!()
    }
    fn clear_clipboard(&mut self) {
        unreachable!()
    }
}

#[test]
fn failed_publication_removes_its_parts_without_removing_a_competing_file() {
    for cancel in [false, true] {
        let temp = Folder::new();
        let dir = temp.path();
        let source = dir.join(b"letter.txt");
        fs::write(std_path(&source), b"first second").unwrap();
        let job = Job::detached(Resolve::Skip);
        let mut publisher = Interrupted { job: cancel.then_some(&job), first: true };
        let split = run(&mut publisher, &source, &dir, Some(5), &job);
        assert!(split.error.is_some());
        assert!(split.items.is_empty());
        assert!(!temp.0.join("letter.001").exists());
        assert!(!temp.0.join("letter.crc").exists());
        if !cancel {
            assert_eq!(fs::read(temp.0.join("letter.002")).unwrap(), b"someone else's file");
        }
    }
}
