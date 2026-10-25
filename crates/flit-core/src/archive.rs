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
        }
    }

    pub fn max_level(self) -> u8 {
        match self {
            Format::Tar | Format::Rar => 0,
            _ => 9,
        }
    }

    pub fn default_level(self) -> u8 {
        match self {
            Format::Tar | Format::Rar => 0,
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

const BROWSABLE: &[(&str, Format)] = &[("zip", Format::Zip), ("7z", Format::SevenZ), ("rar", Format::Rar), ("tar", Format::Tar), ("tgz", Format::TarGz), ("gz", Format::Gz)];

const ZIP_BASED: &[&str] = &["jar", "war", "ear", "apk", "aar", "msix", "appx", "msixbundle", "appxbundle", "nupkg", "vsix", "xpi", "epub", "docx", "xlsx", "pptx", "odt", "ods", "odp"];

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
            Encoding::Cp437 => "CP437 (DOS, Latin US)",
            Encoding::Cp866 => "CP866 (DOS, Cyrillic)",
            Encoding::Cp1251 => "Windows-1251 (Cyrillic)",
            Encoding::Cp1252 => "Windows-1252 (Western)",
            Encoding::Cp1250 => "Windows-1250 (Central European)",
            Encoding::Koi8R => "KOI8-R (Cyrillic)",
            Encoding::ShiftJis => "Shift_JIS (Japanese)",
            Encoding::Gbk => "GBK (Chinese Simplified)",
            Encoding::Big5 => "Big5 (Chinese Traditional)",
            Encoding::EucKr => "EUC-KR (Korean)",
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
}

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
        })
    }
}

pub const PASSWORD_CODE: i64 = 0x5057_0001;
pub const PASSWORD_NEEDED: &str = "The archive is encrypted: enter its password";
pub const WRONG_PASSWORD: &str = "The password is wrong";
pub const INSIDE: &str = "This cannot be done inside an archive";
pub const NO_ADD: &str = "Files can be added only to ZIP archives";

pub fn unreadable(item: &FsPath, meta: Meta) -> Conflict {
    Conflict { source: item.clone(), target: item.clone(), from: meta, to: meta }
}

pub fn is_unreadable(c: &Conflict) -> bool {
    c.source == c.target
}

pub fn needs_password(error: &str) -> bool {
    error.contains(PASSWORD_NEEDED) || error.contains(WRONG_PASSWORD)
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
