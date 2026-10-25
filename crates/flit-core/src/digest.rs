use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HashKind {
    Crc32,
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl HashKind {
    pub const ALL: [HashKind; 6] = [HashKind::Crc32, HashKind::Md5, HashKind::Sha1, HashKind::Sha256, HashKind::Sha384, HashKind::Sha512];

    pub fn label(self) -> &'static str {
        match self {
            HashKind::Crc32 => "CRC32",
            HashKind::Md5 => "MD5",
            HashKind::Sha1 => "SHA-1",
            HashKind::Sha256 => "SHA-256",
            HashKind::Sha384 => "SHA-384",
            HashKind::Sha512 => "SHA-512",
        }
    }

    pub fn hex_len(self) -> usize {
        match self {
            HashKind::Crc32 => 8,
            HashKind::Md5 => 32,
            HashKind::Sha1 => 40,
            HashKind::Sha256 => 64,
            HashKind::Sha384 => 96,
            HashKind::Sha512 => 128,
        }
    }
}

pub fn expected(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_hexdigit()).filter(|t| HashKind::ALL.iter().any(|k| k.hex_len() == t.len())).max_by_key(|t| t.len()).map(str::to_ascii_lowercase)
}

pub fn matching(text: &str, values: &[(HashKind, String)]) -> Option<HashKind> {
    let want = expected(text)?;
    values.iter().find(|(_, v)| v.eq_ignore_ascii_case(&want)).map(|&(k, _)| k)
}

#[cfg(test)]
#[path = "../test/digest.rs"]
mod tests;
