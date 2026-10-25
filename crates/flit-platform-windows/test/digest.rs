use super::*;
use flit_core::provider::Resolve;

struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(tag: &str, body: &[u8]) -> Self {
        let p = std::env::temp_dir().join(format!("flit-digest-{tag}-{}", std::process::id()));
        std::fs::write(&p, body).unwrap();
        Self(p)
    }
    fn path(&self) -> FsPath {
        FsPath::parse(self.0.to_str().unwrap()).unwrap()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn all_six_hashes_match_known_vectors_in_one_pass() {
    let f = TempFile::new("abc", b"abc");
    let job = Job::detached(Resolve::Skip);
    let got = hash_file(&f.path(), &HashKind::ALL, &job).unwrap();
    assert_eq!(
        got,
        [
            "352441c2",
            "900150983cd24fb0d6963f7d28e17f72",
            "a9993e364706816aba3e25717850c26c9cd0d89d",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        ]
    );
    assert_eq!(job.progress().bytes, 3);
}

#[test]
fn large_files_hash_across_chunks_and_cancel_cleanly() {
    let body: Vec<u8> = (0..3_500_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let f = TempFile::new("big", &body);
    let got = hash_file(&f.path(), &[HashKind::Crc32, HashKind::Sha256], &Job::detached(Resolve::Skip)).unwrap();
    assert_eq!(got[0], format!("{:08x}", crc32fast::hash(&body)));
    assert_eq!(got[1].len(), 64);
    let job = Job::detached(Resolve::Skip);
    job.ctl().cancel();
    assert_eq!(hash_file(&f.path(), &[HashKind::Md5], &job).unwrap_err().message, CANCELLED);
    assert!(hash_file(&FsPath::parse(r"C:\definitely\missing\file.bin").unwrap(), &[HashKind::Md5], &Job::detached(Resolve::Skip)).is_err());
}
