use std::collections::HashMap;
use std::path::Path;

const ICONS: &[(u32, &str)] = &[
    (0xE700, "navigation"),
    (0xE70D, "chevron_down"),
    (0xE70E, "chevron_up"),
    (0xE710, "add"),
    (0xE712, "more_horizontal"),
    (0xE713, "settings"),
    (0xE714, "video"),
    (0xE718, "pin"),
    (0xE71B, "link"),
    (0xE71C, "filter"),
    (0xE721, "search"),
    (0xE72A, "arrow_right"),
    (0xE72B, "arrow_left"),
    (0xE72C, "arrow_clockwise"),
    (0xE72D, "share"),
    (0xE738, "subtract"),
    (0xE73E, "checkmark"),
    (0xE74A, "arrow_up"),
    (0xE74D, "delete"),
    (0xE756, "window_console"),
    (0xE767, "speaker_2"),
    (0xE768, "play"),
    (0xE769, "pause"),
    (0xE76B, "chevron_left"),
    (0xE76C, "chevron_right"),
    (0xE777, "arrow_counterclockwise"),
    (0xE77A, "pin_off"),
    (0xE77F, "clipboard_paste"),
    (0xE787, "calendar_ltr"),
    (0xE7A6, "arrow_redo"),
    (0xE7A7, "arrow_undo"),
    (0xE7B8, "box"),
    (0xE7BA, "warning"),
    (0xE7C3, "document"),
    (0xE7F4, "desktop"),
    (0xE80F, "home"),
    (0xE81C, "history"),
    (0xE890, "eye"),
    (0xE896, "arrow_download"),
    (0xE8A0, "panel_right"),
    (0xE8A5, "document_text"),
    (0xE8A9, "grid"),
    (0xE8AC, "rename"),
    (0xE8B3, "select_all_on"),
    (0xE8B7, "folder"),
    (0xE8B9, "image"),
    (0xE8BB, "dismiss"),
    (0xE8C6, "cut"),
    (0xE8C8, "copy"),
    (0xE8CB, "arrow_sort"),
    (0xE8D2, "text_font"),
    (0xE8D6, "music_note_2"),
    (0xE8EC, "tag"),
    (0xE8F1, "library"),
    (0xE8F4, "folder_add"),
    (0xE8FD, "text_bullet_list_ltr"),
    (0xE921, "subtract"),
    (0xE922, "square"),
    (0xE923, "square_multiple"),
    (0xE946, "info"),
    (0xEDA2, "hard_drive"),
];

const SANS_RANGES: &[(u32, u32)] = &[
    (0x20, 0x7E),
    (0xA0, 0x17F),
    (0x300, 0x36F),
    (0x370, 0x3FF),
    (0x400, 0x52F),
    (0x2010, 0x2027),
    (0x2030, 0x203A),
    (0x2044, 0x2044),
    (0x20AC, 0x20AC),
    (0x20BD, 0x20BD),
    (0x2116, 0x2116),
    (0x2122, 0x2122),
    (0x2190, 0x2195),
    (0x2212, 0x2212),
    (0xFFFD, 0xFFFD),
];

pub fn icon_names() -> &'static [(u32, &'static str)] {
    ICONS
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn put16(d: &mut [u8], o: usize, v: u16) {
    d[o..o + 2].copy_from_slice(&v.to_be_bytes());
}

fn tables(data: &[u8]) -> HashMap<[u8; 4], &[u8]> {
    let n = u16_at(data, 4) as usize;
    (0..n)
        .map(|i| {
            let r = 12 + i * 16;
            let tag = data[r..r + 4].try_into().unwrap();
            let (off, len) = (u32_at(data, r + 8) as usize, u32_at(data, r + 12) as usize);
            (tag, &data[off..off + len])
        })
        .collect()
}

struct Source<'a> {
    t: HashMap<[u8; 4], &'a [u8]>,
    long_loca: bool,
}

impl<'a> Source<'a> {
    fn glyph(&self, gid: u16) -> &'a [u8] {
        let (loca, g) = (self.t[b"loca"], gid as usize);
        let (a, b) = if self.long_loca {
            (u32_at(loca, g * 4) as usize, u32_at(loca, g * 4 + 4) as usize)
        } else {
            (u16_at(loca, g * 2) as usize * 2, u16_at(loca, g * 2 + 2) as usize * 2)
        };
        &self.t[b"glyf"][a..b]
    }

    fn metric(&self, gid: u16) -> (u16, u16) {
        let (hmtx, n, g) = (self.t[b"hmtx"], u16_at(self.t[b"hhea"], 34) as usize, gid as usize);
        if g < n {
            (u16_at(hmtx, g * 4), u16_at(hmtx, g * 4 + 2))
        } else {
            (u16_at(hmtx, (n - 1) * 4), u16_at(hmtx, n * 4 + (g - n) * 2))
        }
    }

    fn name(&self, id: u16) -> Option<Vec<u8>> {
        let t = self.t[b"name"];
        let (count, base) = (u16_at(t, 2) as usize, u16_at(t, 4) as usize);
        (0..count).map(|i| 6 + i * 12).find_map(|r| {
            let ok = u16_at(t, r) == 3 && u16_at(t, r + 2) == 1 && u16_at(t, r + 4) == 0x409 && u16_at(t, r + 6) == id;
            ok.then(|| t[base + u16_at(t, r + 10) as usize..][..u16_at(t, r + 8) as usize].to_vec())
        })
    }
}

fn components(g: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if g.len() < 10 || (u16_at(g, 0) as i16) >= 0 {
        return out;
    }
    let mut o = 10;
    loop {
        let flags = u16_at(g, o);
        out.push(o);
        o += 4 + if flags & 1 != 0 { 4 } else { 2 };
        o += if flags & 0x8 != 0 {
            2
        } else if flags & 0x40 != 0 {
            4
        } else if flags & 0x80 != 0 {
            8
        } else {
            0
        };
        if flags & 0x20 == 0 {
            out.push(o);
            return out;
        }
    }
}

fn strip(g: &[u8], new_id: &HashMap<u16, u16>) -> Vec<u8> {
    if g.is_empty() {
        return Vec::new();
    }
    let contours = u16_at(g, 0) as i16;
    let mut out;
    if contours >= 0 {
        let ends = 10 + contours as usize * 2;
        let instr = u16_at(g, ends) as usize;
        out = g[..ends].to_vec();
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&g[ends + 2 + instr..]);
    } else {
        let offs = components(g);
        let end = *offs.last().unwrap();
        out = g[..end].to_vec();
        for &o in &offs[..offs.len() - 1] {
            let flags = u16_at(&out, o) & !0x100;
            put16(&mut out, o, flags);
            let gid = new_id[&u16_at(&out, o + 2)];
            put16(&mut out, o + 2, gid);
        }
    }
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

fn cmap(map: &[(u32, u16)]) -> Vec<u8> {
    let mut segs: Vec<(u16, u16, u16)> = Vec::new();
    for &(cp, gid) in map {
        let cp = u16::try_from(cp).expect("BMP only");
        match segs.last_mut() {
            Some((_, end, delta)) if *end + 1 == cp && delta.wrapping_add(cp) == gid => *end = cp,
            _ => segs.push((cp, cp, gid.wrapping_sub(cp))),
        }
    }
    segs.push((0xFFFF, 0xFFFF, 1));
    let n = segs.len();
    let pow = 1usize << (usize::BITS - 1 - n.leading_zeros());
    let mut sub = Vec::new();
    for v in [4u16, (16 + n * 8) as u16, 0, (n * 2) as u16, (pow * 2) as u16, pow.trailing_zeros() as u16, ((n - pow) * 2) as u16] {
        sub.extend_from_slice(&v.to_be_bytes());
    }
    segs.iter().for_each(|s| sub.extend_from_slice(&s.1.to_be_bytes()));
    sub.extend_from_slice(&[0, 0]);
    segs.iter().for_each(|s| sub.extend_from_slice(&s.0.to_be_bytes()));
    segs.iter().for_each(|s| sub.extend_from_slice(&s.2.to_be_bytes()));
    segs.iter().for_each(|_| sub.extend_from_slice(&[0, 0]));
    let mut out = vec![0, 0, 0, 1, 0, 3, 0, 1, 0, 0, 0, 12];
    out.extend(sub);
    out
}

fn name_table(src: &Source, family: &str) -> Vec<u8> {
    let utf16 = |s: &str| s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect::<Vec<u8>>();
    let ps: String = family.chars().filter(|c| !c.is_whitespace()).collect();
    let mut recs: Vec<(u16, Vec<u8>)> = vec![(1, utf16(family)), (2, utf16("Regular")), (3, utf16(family)), (4, utf16(family)), (6, utf16(&ps))];
    for id in [0, 5, 13, 14] {
        if let Some(s) = src.name(id) {
            recs.push((id, s));
        }
    }
    recs.sort_by_key(|r| r.0);
    let mut out = Vec::new();
    for v in [0u16, recs.len() as u16, (6 + recs.len() * 12) as u16] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut strings = Vec::new();
    for (id, s) in &recs {
        for v in [3u16, 1, 0x409, *id, s.len() as u16, strings.len() as u16] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        strings.extend_from_slice(s);
    }
    out.extend(strings);
    out
}

fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |s, c| {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        s.wrapping_add(u32::from_be_bytes(b))
    })
}

pub fn subset(data: &[u8], map: &[(u32, u16)], family: &str) -> Vec<u8> {
    let t = tables(data);
    let src = Source { long_loca: u16_at(t[b"head"], 50) == 1, t };
    let mut map = map.to_vec();
    map.sort();
    map.dedup_by_key(|m| m.0);
    let mut order: Vec<u16> = vec![0];
    let mut new_id: HashMap<u16, u16> = HashMap::from([(0, 0)]);
    let mut add = |g: u16, order: &mut Vec<u16>| {
        *new_id.entry(g).or_insert_with(|| {
            order.push(g);
            (order.len() - 1) as u16
        })
    };
    let cps: Vec<(u32, u16)> = map.iter().map(|&(cp, g)| (cp, add(g, &mut order))).collect();
    let mut i = 0;
    while i < order.len() {
        let g = src.glyph(order[i]);
        let offs = components(g);
        for &o in offs.iter().take(offs.len().saturating_sub(1)) {
            add(u16_at(g, o + 2), &mut order);
        }
        i += 1;
    }
    let (mut glyf, mut loca, mut hmtx) = (Vec::new(), Vec::new(), Vec::new());
    for &g in &order {
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        glyf.extend(strip(src.glyph(g), &new_id));
        let (adv, lsb) = src.metric(g);
        hmtx.extend_from_slice(&adv.to_be_bytes());
        hmtx.extend_from_slice(&lsb.to_be_bytes());
    }
    loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
    let n = order.len() as u16;
    let mut head = src.t[b"head"].to_vec();
    head[8..12].fill(0);
    put16(&mut head, 50, 1);
    let mut hhea = src.t[b"hhea"].to_vec();
    put16(&mut hhea, 34, n);
    let mut maxp = src.t[b"maxp"].to_vec();
    put16(&mut maxp, 4, n);
    if maxp.len() >= 32 {
        put16(&mut maxp, 26, 0);
    }
    let mut post = src.t[b"post"][..32].to_vec();
    post[..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
    let mut out_tables: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"OS/2", src.t[b"OS/2"].to_vec()),
        (*b"cmap", cmap(&cps)),
        (*b"glyf", glyf),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", maxp),
        (*b"name", name_table(&src, family)),
        (*b"post", post),
    ];
    out_tables.sort_by_key(|t| t.0);
    let count = out_tables.len();
    let pow = 1usize << (usize::BITS - 1 - count.leading_zeros());
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    for v in [count as u16, (pow * 16) as u16, pow.trailing_zeros() as u16, ((count - pow) * 16) as u16] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut offset = 12 + count * 16;
    let mut head_at = 0;
    for (tag, d) in &out_tables {
        if tag == b"head" {
            head_at = offset;
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(d).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(d.len() as u32).to_be_bytes());
        offset += d.len().div_ceil(4) * 4;
    }
    for (_, d) in &out_tables {
        out.extend_from_slice(d);
        out.resize(out.len().div_ceil(4) * 4, 0);
    }
    let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
    out[head_at + 8..head_at + 12].copy_from_slice(&adjust.to_be_bytes());
    out
}

fn glyph_of(data: &[u8], cp: u32) -> u16 {
    swash::FontRef::from_index(data, 0).unwrap().charmap().map(cp)
}

#[test]
#[ignore]
fn regenerate_bundled_fonts() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let fluent = std::env::var("FLIT_FLUENT_DIR").expect("FLIT_FLUENT_DIR with FluentSystemIcons-Regular.ttf and .json");
    let fluent = Path::new(&fluent);
    let ttf = std::fs::read(fluent.join("FluentSystemIcons-Regular.ttf")).unwrap();
    let names: HashMap<String, u32> = serde_json::from_slice(&std::fs::read(fluent.join("FluentSystemIcons-Regular.json")).unwrap()).unwrap();
    let map: Vec<(u32, u16)> = ICONS
        .iter()
        .map(|&(cp, name)| {
            let src = [16, 20].iter().find_map(|s| names.get(&format!("ic_fluent_{name}_{s}_regular"))).unwrap_or_else(|| panic!("{name}"));
            let gid = glyph_of(&ttf, *src);
            assert_ne!(gid, 0, "{name}");
            (cp, gid)
        })
        .collect();
    let icons = subset(&ttf, &map, "Flit Icons");
    std::fs::write(assets.join("FlitIcons.ttf"), &icons).unwrap();
    let dejavu = std::fs::read(std::env::var("FLIT_DEJAVU_TTF").unwrap_or("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into())).unwrap();
    let map: Vec<(u32, u16)> = SANS_RANGES.iter().flat_map(|&(a, b)| a..=b).map(|cp| (cp, glyph_of(&dejavu, cp))).filter(|m| m.1 != 0).collect();
    let sans = subset(&dejavu, &map, "Flit Sans");
    std::fs::write(assets.join("FlitSans.ttf"), &sans).unwrap();
    println!("icons {} bytes, sans {} bytes ({} code points)", icons.len(), sans.len(), map.len());
}
