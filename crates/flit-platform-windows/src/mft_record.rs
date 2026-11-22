pub(crate) const FRN_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
pub(crate) const SECTOR_STRIDE: usize = 512;
pub(crate) const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub(crate) const ATTR_DATA: u32 = 0x80;
pub(crate) const ATTR_REPARSE_POINT: u32 = 0xC0;
pub(crate) const ATTR_END: u32 = 0xFFFF_FFFF;
pub(crate) const RECORD_IN_USE: u16 = 1;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

pub(crate) fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

pub(crate) fn i64_at(b: &[u8], at: usize) -> Option<i64> {
    u64_at(b, at).map(|v| v as i64)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UsnRecord<'a> {
    pub frn: u64,
    pub parent: u64,
    pub usn: i64,
    pub time: i64,
    pub reason: u32,
    pub attrs: u32,
    pub name: &'a [u8],
}

fn narrow_id(rec: &[u8], at: usize) -> Option<Option<u64>> {
    let low = u64_at(rec, at)?;
    let high = u64_at(rec, at + 8)?;
    Some((high == 0).then_some(low))
}

fn usn_record(buf: &[u8]) -> Option<(Option<UsnRecord<'_>>, usize)> {
    let len = u32_at(buf, 0)? as usize;
    if len < 8 || len > buf.len() {
        return None;
    }
    let rec = &buf[..len];
    let id_size = match u16_at(rec, 4)? {
        2 => 8,
        3 => 16,
        _ => return Some((None, len)),
    };
    let parent_at = 8 + id_size;
    let fields = parent_at + id_size;
    let at = usize::from(u16_at(rec, fields + 34)?);
    let name_len = usize::from(u16_at(rec, fields + 32)?);
    let ids = if id_size == 8 { Some((u64_at(rec, 8)?, u64_at(rec, parent_at)?)) } else { narrow_id(rec, 8)?.zip(narrow_id(rec, parent_at)?) };
    let parsed = match ids {
        Some((frn, parent)) => Some(UsnRecord { frn, parent, usn: i64_at(rec, fields)?, time: i64_at(rec, fields + 8)?, reason: u32_at(rec, fields + 16)?, attrs: u32_at(rec, fields + 28)?, name: rec.get(at..at + name_len)? }),
        None => None,
    };
    Some((parsed, len))
}

pub(crate) fn usn_records(mut buf: &[u8]) -> impl Iterator<Item = UsnRecord<'_>> {
    std::iter::from_fn(move || loop {
        let (parsed, len) = usn_record(buf)?;
        buf = &buf[len..];
        if parsed.is_some() {
            return parsed;
        }
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct RecordMeta {
    pub sequence: u16,
    pub base: u64,
    pub size: Option<u64>,
    pub modified: Option<i64>,
    pub reparse_tag: u32,
}

pub(crate) fn fixup(rec: &mut [u8]) -> bool {
    let (Some(at), Some(count)) = (u16_at(rec, 4), u16_at(rec, 6)) else { return false };
    let (at, count) = (at as usize, count as usize);
    if count < 2 || at + count * 2 > rec.len() || (count - 1) * SECTOR_STRIDE > rec.len() {
        return false;
    }
    let check = [rec[at], rec[at + 1]];
    for i in 1..count {
        let end = i * SECTOR_STRIDE - 2;
        if rec[end..end + 2] != check {
            return false;
        }
        rec[end] = rec[at + 2 * i];
        rec[end + 1] = rec[at + 2 * i + 1];
    }
    true
}

fn attributes(rec: &[u8]) -> impl Iterator<Item = (u32, &[u8])> {
    let used = u32_at(rec, 24).map_or(0, |u| (u as usize).min(rec.len()));
    let mut at = u16_at(rec, 20).map_or(usize::MAX, |a| a as usize);
    std::iter::from_fn(move || {
        let ty = u32_at(rec, at)?;
        let len = u32_at(rec, at + 4)? as usize;
        let end = at.checked_add(len)?;
        if ty == ATTR_END || len < 16 || end > used {
            return None;
        }
        let a = &rec[at..end];
        at = end;
        Some((ty, a))
    })
}

fn resident_value(a: &[u8]) -> Option<&[u8]> {
    if a[8] != 0 {
        return None;
    }
    let len = u32_at(a, 16)? as usize;
    let at = u16_at(a, 20)? as usize;
    a.get(at..at.checked_add(len)?)
}

fn unnamed(a: &[u8]) -> bool {
    a[9] == 0
}

fn first_extent(a: &[u8]) -> bool {
    a[8] != 0 && i64_at(a, 16) == Some(0)
}

pub(crate) fn parse_record(rec: &[u8]) -> Option<RecordMeta> {
    if rec.get(..4)? != b"FILE" || u16_at(rec, 22)? & RECORD_IN_USE == 0 {
        return None;
    }
    let mut m = RecordMeta { sequence: u16_at(rec, 16)?, base: u64_at(rec, 32)? & FRN_MASK, ..Default::default() };
    for (ty, a) in attributes(rec) {
        match ty {
            ATTR_STANDARD_INFORMATION => m.modified = resident_value(a).and_then(|v| i64_at(v, 8)).or(m.modified),
            ATTR_DATA if unnamed(a) && a[8] == 0 => m.size = resident_value(a).map(|v| v.len() as u64),
            ATTR_DATA if unnamed(a) && first_extent(a) => m.size = u64_at(a, 48),
            ATTR_REPARSE_POINT => m.reparse_tag = resident_value(a).and_then(|v| u32_at(v, 0)).unwrap_or(0),
            _ => {}
        }
    }
    Some(m)
}

pub(crate) fn data_runs(rec: &[u8]) -> Option<&[u8]> {
    let (_, a) = attributes(rec).find(|&(ty, a)| ty == ATTR_DATA && unnamed(a) && first_extent(a))?;
    a.get(u16_at(a, 32)? as usize..)
}

fn le_unsigned(b: &[u8]) -> u64 {
    b.iter().rev().fold(0, |v, &x| (v << 8) | x as u64)
}

fn le_signed(b: &[u8]) -> i64 {
    let shift = 64 - 8 * b.len() as u32;
    ((le_unsigned(b) << shift) as i64) >> shift
}

pub(crate) fn runs(list: &[u8]) -> Option<Vec<(Option<u64>, u64)>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut lcn = 0i64;
    loop {
        let head = *list.get(at)?;
        if head == 0 {
            return Some(out);
        }
        let (len_size, off_size) = ((head & 0xF) as usize, (head >> 4) as usize);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return None;
        }
        let len = le_unsigned(list.get(at + 1..at + 1 + len_size)?);
        let off = list.get(at + 1 + len_size..at + 1 + len_size + off_size)?;
        at += 1 + len_size + off_size;
        if off_size == 0 {
            out.push((None, len));
            continue;
        }
        lcn = lcn.checked_add(le_signed(off))?;
        if lcn < 0 {
            return None;
        }
        out.push((Some(lcn as u64), len));
    }
}
