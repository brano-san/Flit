use crate::digest::HashKind;
use crate::listing::Meta;
use crate::path::{FsPath, PathKind};
use crate::provider::Conflict;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Format {
    #[default]
    Zip,
    SevenZ,
    Tar,
    Gz,
    TarGz,
    Rar,
    Lzh,
}

impl Format {
    pub const WRITABLE: [Format; 4] = [Format::Zip, Format::SevenZ, Format::Tar, Format::Gz];

    pub fn label(self) -> &'static str {
        match self {
            Format::Zip => "ZIP",
            Format::SevenZ => "7z",
            Format::Tar => "TAR",
            Format::Gz => "GZip",
            Format::TarGz => "TAR.GZ",
            Format::Rar => "RAR",
            Format::Lzh => "LZH",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::SevenZ => "7z",
            Format::Tar => "tar",
            Format::Gz => "gz",
            Format::TarGz => "tar.gz",
            Format::Rar => "rar",
            Format::Lzh => "lzh",
        }
    }

    pub fn max_level(self) -> u8 {
        match self {
            Format::Tar | Format::Rar | Format::Lzh => 0,
            _ => 9,
        }
    }

    pub fn default_level(self) -> u8 {
        match self {
            Format::Tar | Format::Rar | Format::Lzh => 0,
            Format::SevenZ => 5,
            _ => 6,
        }
    }

    pub fn encrypts(self) -> bool {
        matches!(self, Format::Zip | Format::SevenZ)
    }

    pub fn can_add(self) -> bool {
        self == Format::Zip
    }
}

const BROWSABLE: &[(&str, Format)] = &[("zip", Format::Zip), ("7z", Format::SevenZ), ("rar", Format::Rar), ("tar", Format::Tar), ("tgz", Format::TarGz), ("gz", Format::Gz), ("lzh", Format::Lzh), ("lha", Format::Lzh), ("jar", Format::Zip), ("war", Format::Zip), ("ear", Format::Zip), ("apk", Format::Zip), ("aar", Format::Zip), ("msix", Format::Zip), ("appx", Format::Zip), ("msixbundle", Format::Zip), ("appxbundle", Format::Zip), ("nupkg", Format::Zip), ("vsix", Format::Zip), ("xpi", Format::Zip)];

const ZIP_BASED: &[&str] = &["epub", "docx", "xlsx", "pptx", "odt", "ods", "odp"];

fn ext_of(name: &[u8]) -> &[u8] {
    name.iter().rposition(|&c| c == b'.').map_or(&[], |i| &name[i + 1..])
}

fn is_tar_gz(name: &[u8]) -> bool {
    name.len() > 7 && name[name.len() - 7..].eq_ignore_ascii_case(b".tar.gz")
}

pub fn browsable(name: &[u8]) -> Option<Format> {
    if is_tar_gz(name) {
        return Some(Format::TarGz);
    }
    let ext = ext_of(name);
    BROWSABLE.iter().find(|(e, _)| ext.eq_ignore_ascii_case(e.as_bytes())).map(|&(_, f)| f)
}

pub fn format_of(name: &[u8]) -> Option<Format> {
    let ext = ext_of(name);
    browsable(name).or_else(|| ZIP_BASED.iter().any(|e| ext.eq_ignore_ascii_case(e.as_bytes())).then_some(Format::Zip))
}

pub fn stem(name: &[u8]) -> &[u8] {
    let cut = if is_tar_gz(name) { name.len() - 7 } else { name.iter().rposition(|&c| c == b'.').unwrap_or(name.len()) };
    if cut == 0 { name } else { &name[..cut] }
}

pub fn splits(path: &FsPath) -> Vec<(FsPath, Vec<u8>)> {
    let mut chain = Vec::new();
    let mut cur = Some(path.clone());
    while let Some(p) = cur.filter(|p| !p.is_root() && matches!(p.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim | PathKind::Unix)) {
        cur = p.parent();
        chain.push(p);
    }
    let sep = path.sep();
    chain
        .into_iter()
        .rev()
        .filter(|a| format_of(a.file_name()).is_some())
        .map(|a| {
            let rest = path.as_bytes()[a.as_bytes().len()..].iter().skip(1).map(|&c| if c == sep { b'/' } else { c }).collect();
            (a, rest)
        })
        .collect()
}

pub fn split(path: &FsPath) -> Option<(FsPath, Vec<u8>)> {
    splits(path).into_iter().next()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Encoding {
    Utf8,
    Cp437,
    Cp866,
    Cp1251,
    Cp1252,
    Cp1250,
    Koi8R,
    ShiftJis,
    Gbk,
    Big5,
    EucKr,
}

impl Encoding {
    pub const ALL: [Encoding; 11] = [Encoding::Utf8, Encoding::Cp437, Encoding::Cp866, Encoding::Cp1251, Encoding::Cp1252, Encoding::Cp1250, Encoding::Koi8R, Encoding::ShiftJis, Encoding::Gbk, Encoding::Big5, Encoding::EucKr];

    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Cp437 => crate::i18n::S::ArchiveEncCp437.t(),
            Encoding::Cp866 => crate::i18n::S::ArchiveEncCp866.t(),
            Encoding::Cp1251 => crate::i18n::S::ArchiveEncCp1251.t(),
            Encoding::Cp1252 => crate::i18n::S::ArchiveEncCp1252.t(),
            Encoding::Cp1250 => crate::i18n::S::ArchiveEncCp1250.t(),
            Encoding::Koi8R => crate::i18n::S::ArchiveEncKoi8r.t(),
            Encoding::ShiftJis => crate::i18n::S::ArchiveEncShiftJis.t(),
            Encoding::Gbk => crate::i18n::S::ArchiveEncGbk.t(),
            Encoding::Big5 => crate::i18n::S::ArchiveEncBig5.t(),
            Encoding::EucKr => crate::i18n::S::ArchiveEncEucKr.t(),
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Cp437 => "CP437",
            Encoding::Cp866 => "CP866",
            Encoding::Cp1251 => "CP1251",
            Encoding::Cp1252 => "CP1252",
            Encoding::Cp1250 => "CP1250",
            Encoding::Koi8R => "KOI8-R",
            Encoding::ShiftJis => "Shift_JIS",
            Encoding::Gbk => "GBK",
            Encoding::Big5 => "Big5",
            Encoding::EucKr => "EUC-KR",
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret(pub String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compression {
    pub format: Format,
    pub level: u8,
    pub volume: u64,
    pub dictionary: u32,
    pub threads: u32,
    pub password: Option<Secret>,
    pub encrypt_names: bool,
    #[serde(skip)]
    pub word: u32,
}

pub const WORD_SIZES: [u32; 12] = [8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 273];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Place {
    #[default]
    Here,
    OwnFolder,
    Smart,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveOp {
    Compress { items: Vec<FsPath>, archive: FsPath, options: Compression },
    Extract { archives: Vec<FsPath>, dest: Option<FsPath>, place: Place },
    Open { item: FsPath },
}

impl ArchiveOp {
    pub fn map_paths(&self, f: impl Fn(&FsPath) -> Option<FsPath>) -> Option<ArchiveOp> {
        let all = |v: &[FsPath]| v.iter().map(&f).collect::<Option<Vec<_>>>();
        Some(match self {
            ArchiveOp::Compress { items, archive, options } => ArchiveOp::Compress { items: all(items)?, archive: f(archive)?, options: options.clone() },
            ArchiveOp::Extract { archives, dest, place } => ArchiveOp::Extract { archives: all(archives)?, dest: match dest {
                Some(d) => Some(f(d)?),
                None => None,
            }, place: *place },
            ArchiveOp::Open { item } => ArchiveOp::Open { item: f(item)? },
        })
    }
}

pub const HISTORY: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defaults {
    pub compression: Compression,
    pub history: Vec<String>,
    pub open_after: bool,
    pub hashes: Vec<HashKind>,
}

impl Default for Defaults {
    fn default() -> Self {
        let compression = Compression { level: Format::Zip.default_level(), ..Default::default() };
        Self { compression, history: Vec::new(), open_after: false, hashes: HashKind::ALL[..4].to_vec() }
    }
}

impl Defaults {
    pub fn sanitized(mut self) -> Self {
        let c = &mut self.compression;
        if !Format::WRITABLE.contains(&c.format) {
            c.format = Format::Zip;
        }
        c.level = c.level.min(c.format.max_level());
        c.password = None;
        if !WORD_SIZES.contains(&c.word) {
            c.word = 0;
        }
        self.history.truncate(HISTORY);
        self.hashes.dedup();
        if self.hashes.is_empty() {
            self.hashes = Defaults::default().hashes;
        }
        self
    }
}

pub const PASSWORD_CODE: i64 = 0x5057_0001;

pub fn unreadable(item: &FsPath, meta: Meta) -> Conflict {
    Conflict { source: item.clone(), target: item.clone(), from: meta, to: meta }
}

pub fn is_unreadable(c: &Conflict) -> bool {
    c.source == c.target
}

pub fn needs_password(error: &str) -> bool {
    use crate::i18n::S;
    [S::ErrorsArchivePasswordNeeded, S::ErrorsArchiveWrongPassword].iter().any(|s| error.contains(s.en()) || error.contains(s.t()))
}

#[derive(Default)]
struct Remembered {
    key: Vec<u8>,
    encoding: Option<Encoding>,
    detected: Option<Encoding>,
    password: Option<Secret>,
}

const REMEMBERED: usize = 256;

static MEMORY: Mutex<Vec<Remembered>> = Mutex::new(Vec::new());

fn with<R>(archive: &FsPath, f: impl FnOnce(&mut Remembered) -> R) -> R {
    let key = archive.as_bytes().to_ascii_lowercase();
    let mut all = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    let i = match all.iter().position(|r| r.key == key) {
        Some(i) => i,
        None => {
            if all.len() >= REMEMBERED {
                all.remove(0);
            }
            all.push(Remembered { key, ..Default::default() });
            all.len() - 1
        }
    };
    f(&mut all[i])
}

pub fn chosen_encoding(archive: &FsPath) -> Option<Encoding> {
    with(archive, |r| r.encoding)
}

pub fn choose_encoding(archive: &FsPath, encoding: Option<Encoding>) {
    with(archive, |r| r.encoding = encoding);
}

pub fn detected_encoding(archive: &FsPath) -> Option<Encoding> {
    with(archive, |r| r.detected)
}

pub fn set_detected_encoding(archive: &FsPath, encoding: Encoding) {
    with(archive, |r| r.detected = Some(encoding));
}

pub fn password(archive: &FsPath) -> Option<Secret> {
    with(archive, |r| r.password.clone())
}

pub fn set_password(archive: &FsPath, password: Option<Secret>) {
    with(archive, |r| r.password = password);
}

#[cfg(test)]
#[path = "../test/archive.rs"]
mod tests;
