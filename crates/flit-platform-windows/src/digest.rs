use crate::fileops::std_path;
use crossbeam_channel::bounded;
use flit_core::digest::HashKind;
use flit_core::provider::Stage;
use flit_core::FsPath;
use flit_platform::{Job, PlatformError, CANCELLED};
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use windows::Win32::Security::Cryptography::*;

const CHUNK: usize = 1 << 20;
const QUEUE: usize = 8;

enum Digest {
    Crc(crc32fast::Hasher),
    Cng(BCRYPT_HASH_HANDLE, usize),
}

unsafe impl Send for Digest {}

fn status(st: windows::Win32::Foundation::NTSTATUS) -> Result<(), PlatformError> {
    if st.is_ok() { Ok(()) } else { Err(PlatformError::new(st.0 as i64, flit_core::i18n::S::ErrorsEngineHashFailed.en())) }
}

impl Digest {
    fn new(kind: HashKind) -> Result<Self, PlatformError> {
        let (alg, len) = match kind {
            HashKind::Crc32 => return Ok(Digest::Crc(crc32fast::Hasher::new())),
            HashKind::Md5 => (BCRYPT_MD5_ALG_HANDLE, 16),
            HashKind::Sha1 => (BCRYPT_SHA1_ALG_HANDLE, 20),
            HashKind::Sha256 => (BCRYPT_SHA256_ALG_HANDLE, 32),
            HashKind::Sha384 => (BCRYPT_SHA384_ALG_HANDLE, 48),
            HashKind::Sha512 => (BCRYPT_SHA512_ALG_HANDLE, 64),
        };
        let mut h = BCRYPT_HASH_HANDLE::default();
        status(unsafe { BCryptCreateHash(alg, &mut h, None, None, 0) })?;
        Ok(Digest::Cng(h, len))
    }

    fn update(&mut self, data: &[u8]) -> Result<(), PlatformError> {
        match self {
            Digest::Crc(c) => {
                c.update(data);
                Ok(())
            }
            Digest::Cng(h, _) => status(unsafe { BCryptHashData(*h, data, 0) }),
        }
    }

    fn finish(self) -> Result<String, PlatformError> {
        let bytes = match &self {
            Digest::Crc(c) => c.clone().finalize().to_be_bytes().to_vec(),
            Digest::Cng(h, len) => {
                let mut out = vec![0u8; *len];
                status(unsafe { BCryptFinishHash(*h, &mut out, 0) })?;
                out
            }
        };
        Ok(bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        }))
    }
}

impl Drop for Digest {
    fn drop(&mut self) {
        if let Digest::Cng(h, _) = self {
            unsafe {
                let _ = BCryptDestroyHash(*h);
            }
        }
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
    let workers: Vec<_> = kinds
        .iter()
        .map(|&k| {
            let (tx, rx) = bounded::<Arc<Vec<u8>>>(QUEUE);
            let worker = std::thread::Builder::new().name("flit-digest".into()).spawn(move || -> Result<String, PlatformError> {
                let mut d = Digest::new(k)?;
                for chunk in rx {
                    d.update(&chunk)?;
                }
                d.finish()
            });
            (tx, worker)
        })
        .collect();
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
        let value = worker.map_err(io)?.join().map_err(|_| PlatformError::new(-1, flit_core::i18n::S::ErrorsEngineHashFailed.en()))?;
        out.push(value);
    }
    read?;
    let values = out.into_iter().collect::<Result<Vec<_>, _>>()?;
    job.update(|p| p.items = 1);
    Ok(values)
}

#[cfg(test)]
#[path = "../test/digest.rs"]
mod tests;
