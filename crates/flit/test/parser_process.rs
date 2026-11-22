#![cfg(test)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("flit-parser-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn archive(path: &Path) {
    #[cfg(windows)]
    {
        use std::io::Write;
        let mut archive = zip::ZipWriter::new(File::create(path).unwrap());
        archive.start_file("sample.txt", zip::write::SimpleFileOptions::default()).unwrap();
        archive.write_all(b"sample").unwrap();
        archive.finish().unwrap();
    }
    #[cfg(target_os = "linux")]
    {
        let mut archive = tar::Builder::new(File::create(path).unwrap());
        let mut header = tar::Header::new_ustar();
        header.set_mode(0o644);
        header.set_size(6);
        header.set_cksum();
        archive.append_data(&mut header, "sample.txt", &b"sample"[..]).unwrap();
        archive.finish().unwrap();
    }
}

fn helper(task: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flit")).args([flit_platform::parse_process::SUBCOMMAND, task]).arg(path).args(args).output().unwrap()
}

#[test]
fn the_archive_preview_subcommand_reads_names_and_handles_damaged_archives() {
    let dir = TestDir::new();
    let path = dir.0.join(if cfg!(windows) { "sample.zip" } else { "sample.tar" });
    archive(&path);
    let output = helper("archive", &path, &[]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout.strip_prefix(flit_platform::parse_process::ARCHIVE_HEADER), Some(&b"sample.txt\n"[..]));

    std::fs::write(&path, b"damaged archive").unwrap();
    let output = helper("archive", &path, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn a_preview_reports_size_and_password_limits_distinctly_from_corruption() {
    use flit_core::archive::{ArchiveOp, Compression, Format, Secret};
    use flit_core::provider::{FileOp, Resolve};
    use flit_core::FsPath;

    let dir = TestDir::new();
    let large = dir.0.join("large.tar");
    let mut tar = File::create(&large).unwrap();
    use std::io::Write;
    tar.write_all(&[0; 1024]).unwrap();
    tar.set_len((256 << 20) + 512).unwrap();
    drop(tar);
    let output = helper("archive", &large, &[]);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());

    let input = dir.0.join("locked.txt");
    let encrypted = dir.0.join("encrypted.7z");
    std::fs::write(&input, b"secret text").unwrap();
    let path = |p: &Path| FsPath::parse(p.to_str().unwrap()).unwrap();
    let op = FileOp::Archive(ArchiveOp::Compress { items: vec![path(&input)], archive: path(&encrypted), options: Compression { format: Format::SevenZ, password: Some(Secret("secret".into())), encrypt_names: true, ..Compression::default() } });
    let env = flit_archive::Env { utc_offset: 0, free_space: &|_| Some(u64::MAX) };
    let result = flit_archive::file_op(&op, &flit_platform::Job::detached(Resolve::Replace), &env).unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    let output = helper("archive", &encrypted, &[]);
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
}

#[test]
#[cfg(target_os = "linux")]
fn a_webp_decoder_rejection_has_its_own_exit_status() {
    let dir = TestDir::new();
    let webp = dir.0.join("broken.webp");
    std::fs::write(&webp, b"RIFF\x04\0\0\0WEBP").unwrap();
    let output = helper("webp", &webp, &["96"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    assert_eq!(helper("webp", &webp, &["invalid"]).status.code(), Some(2));
}

#[test]
fn probing_parser_startup_does_not_open_the_input() {
    let dir = TestDir::new();
    let missing = dir.0.join("missing.zip");
    let probe = helper(flit_platform::parse_process::PROBE_TASK, &missing, &[]);
    assert!(probe.status.success());
    assert!(probe.stdout.is_empty());
    assert_eq!(helper(flit_platform::parse_process::PROBE_TASK, &missing, &["extra"]).status.code(), Some(2));
}
