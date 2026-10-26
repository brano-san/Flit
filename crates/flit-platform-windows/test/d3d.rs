use super::*;

fn chunks(png: &[u8]) -> Vec<(String, Vec<u8>)> {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let mut out = Vec::new();
    let mut at = 8;
    while at < png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        let body = &png[at + 4..at + 8 + len];
        assert_eq!(u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap()), crc32fast::hash(body));
        out.push((String::from_utf8_lossy(&body[..4]).into_owned(), body[4..].to_vec()));
        at += 12 + len;
    }
    out
}

fn stored(z: &[u8]) -> Vec<u8> {
    let mut raw = Vec::new();
    let mut at = 2;
    loop {
        let last = z[at] & 1 == 1;
        let n = u16::from_le_bytes([z[at + 1], z[at + 2]]) as usize;
        assert_eq!(u16::from_le_bytes([z[at + 3], z[at + 4]]), !(n as u16));
        raw.extend_from_slice(&z[at + 5..at + 5 + n]);
        at += 5 + n;
        if last {
            assert_eq!(at + 4, z.len());
            return raw;
        }
    }
}

#[test]
fn png_round_trips_its_rows_and_a_zero_size_window_gives_one_black_pixel() {
    let raw = [0, 1, 2, 3, 0, 4, 5, 6];
    let c = chunks(&png(1, 2, &raw));
    assert_eq!(c.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["IHDR", "IDAT", "IEND"]);
    assert_eq!(&c[0].1[..8], &[0, 0, 0, 1, 0, 0, 0, 2]);
    assert_eq!(stored(&c[1].1), raw);
    for (w, h) in [(0, 0), (0, 5), (5, 0)] {
        let c = chunks(&png(w, h, &[]));
        assert_eq!(&c[0].1[..8], &[0, 0, 0, 1, 0, 0, 0, 1]);
        assert_eq!(stored(&c[1].1), [0, 0, 0, 0]);
    }
}
