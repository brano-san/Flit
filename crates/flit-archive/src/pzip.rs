use crate::compress::{unreadable, write_error, zip_time, Source};
use crate::extract::cancelled;
use crate::{corrupt, Env};
use flate2::{Compress, Compression, FlushCompress, Status};
use flit_platform::{Job, PlatformError};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

const CHUNK: usize = 1 << 20;
const DICT: usize = 32 << 10;
const MAX_PIECES: usize = 4096;
const BIG: u64 = 0xFFFF_0000;
const FULL: u64 = u32::MAX as u64;

#[derive(Default)]
struct Piece {
    entry: usize,
    first: bool,
    last: bool,
    dict: Vec<u8>,
    data: Vec<u8>,
    out: Vec<u8>,
    crc: crc32fast::Hasher,
}

struct Feed<'a> {
    sources: &'a [Source],
    next: usize,
    open: Option<(usize, File, Vec<u8>, u64)>,
    job: &'a Job,
}

fn read_full(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

impl Feed<'_> {
    fn fill(&mut self, budget: usize) -> Result<Vec<Piece>, PlatformError> {
        let mut pieces = Vec::new();
        let mut bytes = 0;
        while bytes < budget && pieces.len() < MAX_PIECES {
            if self.job.cancelled() {
                return Err(cancelled());
            }
            if let Some((entry, mut file, dict, done)) = self.open.take() {
                let want = CHUNK.min(self.sources[entry].size.saturating_sub(done) as usize + 1);
                let mut data = vec![0; want];
                let n = read_full(&mut file, &mut data).map_err(write_error)?;
                data.truncate(n);
                let last = n < want;
                if !last {
                    self.open = Some((entry, file, data[n.saturating_sub(DICT)..].to_vec(), done + n as u64));
                }
                bytes += n;
                pieces.push(Piece { entry, first: done == 0, last, dict, data, ..Piece::default() });
                continue;
            }
            let Some(s) = self.sources.get(self.next) else { break };
            let entry = self.next;
            self.next += 1;
            if s.dir {
                pieces.push(Piece { entry, first: true, last: true, ..Piece::default() });
                continue;
            }
            match File::open(&s.path) {
                Ok(f) => self.open = Some((entry, f, Vec::new(), 0)),
                Err(_) => unreadable(s, self.job)?,
            }
        }
        Ok(pieces)
    }
}

fn deflate(c: &mut Compress, p: &mut Piece) -> Result<(), PlatformError> {
    let fail = |e: flate2::CompressError| corrupt(&e.to_string());
    c.reset();
    if !p.dict.is_empty() {
        c.set_dictionary(&p.dict).map_err(fail)?;
    }
    let flush = if p.last { FlushCompress::Finish } else { FlushCompress::Sync };
    p.out = Vec::with_capacity(p.data.len() / 2 + 4096);
    loop {
        if p.out.len() == p.out.capacity() {
            p.out.reserve(p.out.capacity());
        }
        let used = c.total_in() as usize;
        let status = c.compress_vec(&p.data[used..], &mut p.out, flush).map_err(fail)?;
        let drained = c.total_in() as usize == p.data.len() && p.out.len() < p.out.capacity();
        if status == Status::StreamEnd || (!p.last && drained) {
            return Ok(());
        }
    }
}

fn pack(pieces: &mut [Piece], level: u8, threads: usize, store: bool) -> Result<(), PlatformError> {
    let queue = Mutex::new(pieces.iter_mut());
    let failure = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut c = Compress::new(Compression::new(level.min(9) as u32), false);
                loop {
                    let next = queue.lock().unwrap_or_else(|e| e.into_inner()).next();
                    let Some(p) = next else { return };
                    p.crc.update(&p.data);
                    if store {
                        continue;
                    }
                    if let Err(e) = deflate(&mut c, p) {
                        *failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                        return;
                    }
                }
            });
        }
    });
    failure.into_inner().unwrap_or_else(|e| e.into_inner()).map_or(Ok(()), Err)
}

struct Entry {
    name: Vec<u8>,
    dir: bool,
    time: u16,
    date: u16,
    method: u16,
    zip64: bool,
    offset: u64,
    crc: crc32fast::Hasher,
    csize: u64,
    size: u64,
}

impl Entry {
    fn version(&self) -> u16 {
        if self.zip64 || self.offset >= FULL { 45 } else { 20 }
    }

    fn fixed(&self, out: &mut Vec<u8>) {
        out.extend(self.version().to_le_bytes());
        out.extend(0x0800u16.to_le_bytes());
        out.extend(self.method.to_le_bytes());
        out.extend(self.time.to_le_bytes());
        out.extend(self.date.to_le_bytes());
        out.extend(self.crc.clone().finalize().to_le_bytes());
        let (csize, size) = if self.zip64 { (FULL, FULL) } else { (self.csize, self.size) };
        out.extend((csize as u32).to_le_bytes());
        out.extend((size as u32).to_le_bytes());
        out.extend((self.name.len() as u16).to_le_bytes());
    }

    fn local(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(50 + self.name.len());
        h.extend(0x0403_4b50u32.to_le_bytes());
        self.fixed(&mut h);
        h.extend((if self.zip64 { 20u16 } else { 0 }).to_le_bytes());
        h.extend(&self.name);
        if self.zip64 {
            h.extend(1u16.to_le_bytes());
            h.extend(16u16.to_le_bytes());
            h.extend(self.size.to_le_bytes());
            h.extend(self.csize.to_le_bytes());
        }
        h
    }

    fn central(&self) -> Vec<u8> {
        let mut extra = Vec::new();
        if self.zip64 {
            extra.extend(self.size.to_le_bytes());
            extra.extend(self.csize.to_le_bytes());
        }
        if self.offset >= FULL {
            extra.extend(self.offset.to_le_bytes());
        }
        let mut h = Vec::with_capacity(70 + self.name.len());
        h.extend(0x0201_4b50u32.to_le_bytes());
        h.extend(45u16.to_le_bytes());
        self.fixed(&mut h);
        h.extend((if extra.is_empty() { 0 } else { extra.len() as u16 + 4 }).to_le_bytes());
        h.extend([0; 6]);
        h.extend((if self.dir { 0x10u32 } else { 0 }).to_le_bytes());
        h.extend((self.offset.min(FULL) as u32).to_le_bytes());
        h.extend(&self.name);
        if !extra.is_empty() {
            h.extend(1u16.to_le_bytes());
            h.extend((extra.len() as u16).to_le_bytes());
            h.extend(extra);
        }
        h
    }
}

struct Out<W> {
    w: W,
    pos: u64,
    method: u16,
    entries: Vec<Entry>,
}

impl<W: Write + Seek> Out<W> {
    fn emit(&mut self, b: &[u8]) -> Result<(), PlatformError> {
        self.w.write_all(b).map_err(write_error)?;
        self.pos += b.len() as u64;
        Ok(())
    }

    fn put(&mut self, p: Piece, s: &Source, job: &Job, env: &Env<'_>) -> Result<(), PlatformError> {
        if p.first {
            let mut name = s.name.clone().into_bytes();
            if s.dir {
                name.push(b'/');
            }
            let at = zip_time(s.modified, env.utc_offset);
            let method = if s.dir { 0 } else { self.method };
            let crc = crc32fast::Hasher::new();
            self.entries.push(Entry { name, dir: s.dir, time: at.timepart(), date: at.datepart(), method, zip64: !s.dir && s.size >= BIG, offset: self.pos, crc, csize: 0, size: 0 });
        }
        let body = if self.method == 0 || s.dir { &p.data } else { &p.out };
        let e = self.entries.last_mut().expect("an entry started by its first piece");
        e.crc.combine(&p.crc);
        e.csize += body.len() as u64;
        e.size += p.data.len() as u64;
        if p.last && !e.zip64 && e.csize.max(e.size) >= FULL {
            return Err(corrupt("a file grew past 4 GB while it was being compressed"));
        }
        if p.first {
            let head = e.local();
            self.emit(&head)?;
        }
        self.emit(body)?;
        job.update(|q| q.bytes += p.data.len() as u64);
        if p.last {
            if !p.first {
                let e = self.entries.last().expect("an entry started by its first piece");
                self.w.seek(SeekFrom::Start(e.offset)).map_err(write_error)?;
                self.w.write_all(&e.local()).map_err(write_error)?;
                self.w.seek(SeekFrom::Start(self.pos)).map_err(write_error)?;
            }
            job.update(|q| q.items += 1);
        }
        Ok(())
    }

    fn finish(mut self) -> Result<W, PlatformError> {
        let start = self.pos;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            self.emit(&e.central())?;
        }
        let (n, size) = (entries.len() as u64, self.pos - start);
        let mut end = Vec::with_capacity(98);
        if n >= 0xFFFF || start >= FULL || size >= FULL {
            end.extend(0x0606_4b50u32.to_le_bytes());
            end.extend(44u64.to_le_bytes());
            end.extend(45u16.to_le_bytes());
            end.extend(45u16.to_le_bytes());
            end.extend([0; 8]);
            end.extend(n.to_le_bytes());
            end.extend(n.to_le_bytes());
            end.extend(size.to_le_bytes());
            end.extend(start.to_le_bytes());
            end.extend(0x0706_4b50u32.to_le_bytes());
            end.extend(0u32.to_le_bytes());
            end.extend(self.pos.to_le_bytes());
            end.extend(1u32.to_le_bytes());
        }
        end.extend(0x0605_4b50u32.to_le_bytes());
        end.extend([0; 4]);
        end.extend((n.min(0xFFFF) as u16).to_le_bytes());
        end.extend((n.min(0xFFFF) as u16).to_le_bytes());
        end.extend((size.min(FULL) as u32).to_le_bytes());
        end.extend((start.min(FULL) as u32).to_le_bytes());
        end.extend(0u16.to_le_bytes());
        self.emit(&end)?;
        Ok(self.w)
    }
}

pub(crate) fn write<W: Write + Seek>(w: W, sources: &[Source], level: u8, threads: usize, job: &Job, env: &Env<'_>) -> Result<W, PlatformError> {
    let mut feed = Feed { sources, next: 0, open: None, job };
    let mut out = Out { w, pos: 0, method: if level == 0 { 0 } else { 8 }, entries: Vec::new() };
    let budget = threads.max(1) * 4 * CHUNK;
    let mut batch = feed.fill(budget)?;
    while !batch.is_empty() {
        let (packed, next) = std::thread::scope(|s| {
            let reader = s.spawn(|| feed.fill(budget));
            let packed = pack(&mut batch, level, threads.max(1), level == 0);
            (packed, reader.join().unwrap_or_else(|_| Err(corrupt("the archive reader stopped"))))
        });
        packed?;
        for p in batch {
            let s = &sources[p.entry];
            out.put(p, s, job, env)?;
        }
        batch = next?;
    }
    out.finish()
}

pub(crate) struct Gzip<W: Write> {
    w: W,
    level: u8,
    threads: usize,
    pieces: Vec<Piece>,
    buf: Vec<u8>,
    tail: Vec<u8>,
    crc: crc32fast::Hasher,
    len: u64,
}

impl<W: Write> Gzip<W> {
    pub(crate) fn new(mut w: W, level: u8, threads: usize, name: Option<&[u8]>, mtime: u32) -> io::Result<Self> {
        let mut head = vec![0x1f, 0x8b, 8, if name.is_some() { 8 } else { 0 }];
        head.extend(mtime.to_le_bytes());
        head.extend([0, 255]);
        if let Some(name) = name {
            head.extend(name.iter().filter(|&&b| b != 0));
            head.push(0);
        }
        w.write_all(&head)?;
        let threads = threads.max(1);
        Ok(Self { w, level, threads, pieces: Vec::with_capacity(threads * 4), buf: Vec::with_capacity(CHUNK), tail: Vec::new(), crc: crc32fast::Hasher::new(), len: 0 })
    }

    fn cut(&mut self, last: bool) {
        let data = std::mem::replace(&mut self.buf, if last { Vec::new() } else { Vec::with_capacity(CHUNK) });
        let dict = std::mem::replace(&mut self.tail, data[data.len().saturating_sub(DICT)..].to_vec());
        self.pieces.push(Piece { last, dict, data, ..Piece::default() });
    }

    fn drain(&mut self) -> io::Result<()> {
        pack(&mut self.pieces, self.level, self.threads, false).map_err(|e| io::Error::other(format!("{e:?}")))?;
        for p in self.pieces.drain(..) {
            self.w.write_all(&p.out)?;
            self.crc.combine(&p.crc);
            self.len += p.data.len() as u64;
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> io::Result<W> {
        self.cut(true);
        self.drain()?;
        self.w.write_all(&self.crc.clone().finalize().to_le_bytes())?;
        self.w.write_all(&(self.len as u32).to_le_bytes())?;
        Ok(self.w)
    }
}

impl<W: Write> Write for Gzip<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = buf.len().min(CHUNK - self.buf.len());
        self.buf.extend_from_slice(&buf[..n]);
        if self.buf.len() == CHUNK {
            self.cut(false);
            if self.pieces.len() == self.threads * 4 {
                self.drain()?;
            }
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}
