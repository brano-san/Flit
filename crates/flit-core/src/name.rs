use std::borrow::Cow;

#[inline]
pub fn is_delete_stage(name: &[u8]) -> bool {
    let Some(id) = name.strip_prefix(b".flit-deleting-") else { return false };
    let mut fields = id.split(|&byte| byte == b'-');
    let valid = |part: Option<&[u8]>, check: fn(&u8) -> bool| part.is_some_and(|part| !part.is_empty() && part.iter().all(check));
    valid(fields.next(), u8::is_ascii_digit) && valid(fields.next(), u8::is_ascii_hexdigit)
        && valid(fields.next(), u8::is_ascii_hexdigit) && fields.next().is_none()
}

pub fn is_operation_stage(name: &[u8]) -> bool {
    (name.starts_with(b".flit-stage-") && name.ends_with(b".flit-partial")) || is_delete_stage(name)
}

pub fn push_utf16(out: &mut Vec<u8>, src: &[u16]) {
    out.reserve(src.len());
    let mut i = 0;
    while i < src.len() {
        let u = src[i];
        i += 1;
        if u < 0x80 {
            out.push(u as u8);
            continue;
        }
        let mut cp = u as u32;
        if (0xD800..0xDC00).contains(&u) && i < src.len() && (0xDC00..0xE000).contains(&src[i]) {
            cp = 0x10000 + (((u as u32 - 0xD800) << 10) | (src[i] as u32 - 0xDC00));
            i += 1;
        }
        push_cp(out, cp);
    }
}

#[inline]
pub fn push_cp(out: &mut Vec<u8>, cp: u32) {
    if cp < 0x80 {
        out.push(cp as u8);
    } else if cp < 0x800 {
        out.extend_from_slice(&[0xC0 | (cp >> 6) as u8, 0x80 | (cp & 0x3F) as u8]);
    } else if cp < 0x10000 {
        out.extend_from_slice(&[
            0xE0 | (cp >> 12) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]);
    } else {
        out.extend_from_slice(&[
            0xF0 | (cp >> 18) as u8,
            0x80 | ((cp >> 12) & 0x3F) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]);
    }
}

#[inline]
pub fn decode(b: &[u8]) -> (u32, usize) {
    let b0 = b[0];
    if b0 < 0x80 {
        return (b0 as u32, 1);
    }
    let cont = |i: usize| b.get(i).copied().filter(|&c| c & 0xC0 == 0x80).map(|c| (c & 0x3F) as u32);
    let (len, init) = match b0 {
        0xC2..=0xDF => (2, (b0 & 0x1F) as u32),
        0xE0..=0xEF => (3, (b0 & 0x0F) as u32),
        0xF0..=0xF4 => (4, (b0 & 0x07) as u32),
        _ => return (0xFFFD, 1),
    };
    let mut cp = init;
    for i in 1..len {
        match cont(i) {
            Some(c) => cp = (cp << 6) | c,
            None => return (0xFFFD, 1),
        }
    }
    let min = [0, 0, 0x80, 0x800, 0x10000][len];
    if cp < min || cp > 0x10FFFF {
        return (0xFFFD, 1);
    }
    (cp, len)
}

pub fn to_utf16(src: &[u8], out: &mut Vec<u16>) {
    out.reserve(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] < 0x80 {
            out.push(src[i] as u16);
            i += 1;
            continue;
        }
        let (cp, n) = decode(&src[i..]);
        i += n;
        if cp >= 0x10000 {
            let c = cp - 0x10000;
            out.push(0xD800 | (c >> 10) as u16);
            out.push(0xDC00 | (c & 0x3FF) as u16);
        } else {
            out.push(cp as u16);
        }
    }
}

pub fn display(b: &[u8]) -> Cow<'_, str> {
    if let Ok(s) = std::str::from_utf8(b) {
        return Cow::Borrowed(s);
    }
    let mut s = String::with_capacity(b.len() + 4);
    let mut i = 0;
    while i < b.len() {
        let (cp, n) = decode(&b[i..]);
        s.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
        i += n;
    }
    Cow::Owned(s)
}

pub fn starts_with_ci(name: &[u8], prefix: &str) -> bool {
    let name = display(name);
    let mut n = name.chars().flat_map(char::to_lowercase);
    prefix.chars().flat_map(char::to_lowercase).all(|p| n.next() == Some(p))
}

#[inline]
pub fn ext_len(name: &[u8]) -> usize {
    match memrchr(b'.', name) {
        Some(p) if p + 1 < name.len() => name.len() - p - 1,
        _ => 0,
    }
}

pub fn link_name(target: &[u8]) -> Vec<u8> {
    link_name_in(target, crate::i18n::lang())
}

pub fn link_name_in(target: &[u8], lang: crate::i18n::Lang) -> Vec<u8> {
    let pattern = crate::i18n::S::OpsExtraLinkName.in_lang(lang.catalogue());
    let (before, after) = pattern.split_once("{name}").unwrap_or((pattern, ""));
    [before.as_bytes(), target, after.as_bytes()].concat()
}

pub fn copy_suffix_in(lang: crate::i18n::Lang) -> &'static str {
    let pattern = crate::i18n::S::OpsExtraCopyName.in_lang(lang.catalogue());
    pattern.split_once("{name}").map_or(pattern, |(_, after)| after)
}

#[inline]
fn memrchr(c: u8, s: &[u8]) -> Option<usize> {
    s.iter().rposition(|&b| b == c)
}

#[cfg(test)]
#[path = "../test/name.rs"]
mod tests;
pub(crate) fn percent_decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}
