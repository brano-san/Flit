use std::io::{Read, Seek, SeekFrom};

const MAX_IFDS: usize = 32;
const MAX_ENTRIES: usize = 256;
const MAX_SUB_IFDS: usize = 16;
pub(crate) const MAX_JPEG_BYTES: u32 = 8 << 20;

#[derive(Clone, Copy)]
enum Order {
    Le,
    Be,
}

impl Order {
    fn u16(self, src: &[u8]) -> u16 {
        let bytes = [src[0], src[1]];
        match self { Self::Le => u16::from_le_bytes(bytes), Self::Be => u16::from_be_bytes(bytes) }
    }

    fn u32(self, src: &[u8]) -> u32 {
        let bytes = [src[0], src[1], src[2], src[3]];
        match self { Self::Le => u32::from_le_bytes(bytes), Self::Be => u32::from_be_bytes(bytes) }
    }
}

fn in_file(size: u64, at: u32, len: u32) -> bool {
    at >= 8 && u64::from(at) + u64::from(len) <= size
}

fn read_at<R: Read + Seek>(r: &mut R, size: u64, at: u32, buf: &mut [u8]) -> Option<()> {
    if !in_file(size, at, buf.len().try_into().ok()?) {
        return None;
    }
    r.seek(SeekFrom::Start(u64::from(at))).ok()?;
    r.read_exact(buf).ok()
}

fn queue(offsets: &mut Vec<u32>, at: u32) {
    if at >= 8 && offsets.len() < MAX_IFDS && !offsets.contains(&at) {
        offsets.push(at);
    }
}

type JpegLocation = (u32, u32);

pub(crate) fn locations<R: Read + Seek>(r: &mut R, size: u64, current: impl Fn() -> bool) -> Option<(u16, Vec<JpegLocation>)> {
    let mut header = [0; 8];
    r.read_exact(&mut header).ok()?;
    let order = match &header[..2] {
        b"II" => Order::Le,
        b"MM" => Order::Be,
        _ => return None,
    };
    let magic = order.u16(&header[2..]);
    if !matches!(magic, 42 | 85 | 0x4f52 | 0x5352) {
        return None;
    }
    let mut offsets = Vec::with_capacity(MAX_IFDS);
    queue(&mut offsets, order.u32(&header[4..]));
    let mut items = Vec::new();
    let mut orientation = 1;
    let mut index = 0;
    let mut storage = [0; MAX_ENTRIES * 12 + 4];
    while let Some(&at) = offsets.get(index) {
        index += 1;
        if !current() {
            return None;
        }
        let mut count = [0; 2];
        if read_at(r, size, at, &mut count).is_none() {
            continue;
        }
        let count = usize::from(order.u16(&count));
        if count > MAX_ENTRIES {
            continue;
        }
        let bytes = &mut storage[..count * 12 + 4];
        if at.checked_add(2).and_then(|pos| read_at(r, size, pos, bytes)).is_none() {
            continue;
        }
        queue(&mut offsets, order.u32(&bytes[count * 12..]));
        let (mut jpeg, mut strips, mut tiles) = ((0, 0), (0, 0), (0, 0));
        let mut compression = 0;
        for e in bytes[..count * 12].chunks_exact(12) {
            let tag = order.u16(e);
            let (kind, n, value) = (order.u16(&e[2..]), order.u32(&e[4..]), order.u32(&e[8..]));
            if tag == 330 && matches!(kind, 4 | 13) && (1..=MAX_SUB_IFDS as u32).contains(&n) {
                if n == 1 {
                    queue(&mut offsets, value);
                } else {
                    let mut sub = [0; MAX_SUB_IFDS * 4];
                    let range = &mut sub[..n as usize * 4];
                    if read_at(r, size, value, range).is_some() {
                        for offset in range.chunks_exact(4) {
                            queue(&mut offsets, order.u32(offset));
                        }
                    }
                }
            } else if magic == 85 && tag == 0x2e && matches!(kind, 1 | 7) && n > 4 {
                jpeg = (value, n);
            } else if n == 1 && matches!(kind, 3 | 4) {
                let value = if kind == 3 { u32::from(order.u16(&e[8..])) } else { value };
                match tag {
                    259 => compression = value,
                    273 => strips.0 = value,
                    274 if index == 1 && (1..=8).contains(&value) => orientation = value as u16,
                    279 => strips.1 = value,
                    324 => tiles.0 = value,
                    325 => tiles.1 = value,
                    513 => jpeg.0 = value,
                    514 => jpeg.1 = value,
                    _ => {}
                }
            }
        }
        let other = if matches!(compression, 6 | 7) { [strips, tiles] } else { [(0, 0); 2] };
        for (offset, len) in [jpeg, other[0], other[1]] {
            if (4..=MAX_JPEG_BYTES).contains(&len) && in_file(size, offset, len) && !items.contains(&(offset, len)) {
                let mut magic = [0; 3];
                if read_at(r, size, offset, &mut magic).is_some() && magic == [0xFF, 0xD8, 0xFF] {
                    items.push((offset, len));
                }
            }
        }
    }
    Some((orientation, items))
}
