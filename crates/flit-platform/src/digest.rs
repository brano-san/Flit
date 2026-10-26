use crate::{std_path, Job, PlatformError, CANCELLED};
use crossbeam_channel::bounded;
use flit_core::digest::HashKind;
use flit_core::provider::Stage;
use flit_core::FsPath;
use sha2::digest::DynDigest;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;

const CHUNK: usize = 1 << 20;
const QUEUE: usize = 8;

enum Hasher {
    Crc(crc32fast::Hasher),
    Digest(Box<dyn DynDigest + Send>),
}

impl Hasher {
    fn new(kind: HashKind) -> Self {
        match kind {
            HashKind::Crc32 => Hasher::Crc(crc32fast::Hasher::new()),
            HashKind::Md5 => Hasher::Digest(Box::new(md5::Md5::default())),
            HashKind::Sha1 => Hasher::Digest(Box::new(sha1::Sha1::default())),
            HashKind::Sha256 => Hasher::Digest(Box::new(sha2::Sha256::default())),
            HashKind::Sha384 => Hasher::Digest(Box::new(sha2::Sha384::default())),
            HashKind::Sha512 => Hasher::Digest(Box::new(sha2::Sha512::default())),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Crc(c) => c.update(data),
            Hasher::Digest(d) => d.update(data),
        }
    }

    fn finish(self) -> String {
        let bytes = match self {
            Hasher::Crc(c) => c.finalize().to_be_bytes().to_vec(),
            Hasher::Digest(d) => d.finalize().into_vec(),
        };
        bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }
}

fn fill(f: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

pub fn hash_file(file: &FsPath, kinds: &[HashKind], job: &Job) -> Result<Vec<String>, PlatformError> {
    let io = |e: std::io::Error| PlatformError::new(e.raw_os_error().map_or(-1, i64::from), e.to_string());
    let mut f = File::open(std_path(file)).map_err(io)?;
    let total = f.metadata().map_err(io)?.len();
    job.update(|p| {
        p.stage = Stage::Running;
        p.total_bytes = total;
        p.total_items = 1;
    });
    let workers = kinds
        .iter()
        .map(|&k| {
            let (tx, rx) = bounded::<Arc<Vec<u8>>>(QUEUE);
            let worker = std::thread::Builder::new().name("flit-digest".into()).spawn(move || {
                let mut h = Hasher::new(k);
                for chunk in rx {
                    h.update(&chunk);
                }
                h.finish()
            });
            worker.map(|w| (tx, w)).map_err(io)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut read = Ok(());
    loop {
        if job.cancelled() {
            read = Err(PlatformError::new(-1, CANCELLED));
            break;
        }
        let mut buf = vec![0u8; CHUNK];
        let n = match fill(&mut f, &mut buf) {
            Ok(n) => n,
            Err(e) => {
                read = Err(io(e));
                break;
            }
        };
        if n == 0 {
            break;
        }
        buf.truncate(n);
        let chunk = Arc::new(buf);
        if workers.iter().any(|(tx, _)| tx.send(chunk.clone()).is_err()) {
            break;
        }
        job.update(|p| p.bytes += n as u64);
    }
    let mut out = Vec::with_capacity(kinds.len());
    for (tx, worker) in workers {
        drop(tx);
        out.push(worker.join().map_err(|_| PlatformError::new(-1, flit_core::i18n::S::ErrorsEngineHashFailed.en()))?);
    }
    read?;
    job.update(|p| p.items = 1);
    Ok(out)
}

#[cfg(test)]
#[path = "../test/digest.rs"]
mod tests;
