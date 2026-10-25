use super::*;
use crate::com_sta;
use std::path::PathBuf;

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> Dir {
    let d = std::env::temp_dir().join(format!("flit-preview-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Dir(d)
}

fn fs(p: &std::path::Path) -> FsPath {
    FsPath::parse(p.to_str().unwrap()).unwrap()
}

pub(crate) fn bmp(w: u32, h: u32, bgr: [u8; 3]) -> Vec<u8> {
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut b = Vec::with_capacity(size as usize);
    b.extend(b"BM");
    b.extend(size.to_le_bytes());
    b.extend([0u8; 4]);
    b.extend(54u32.to_le_bytes());
    b.extend(40u32.to_le_bytes());
    b.extend(w.to_le_bytes());
    b.extend(h.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(24u16.to_le_bytes());
    b.extend([0u8; 24]);
    for _ in 0..h {
        for _ in 0..w {
            b.extend(bgr);
        }
        b.extend(vec![0u8; (row - w * 3) as usize]);
    }
    b
}

#[test]
fn decodes_and_scales_images_into_premultiplied_rgba() {
    com_sta();
    let d = temp("wic");
    let p = d.0.join("red.bmp");
    std::fs::write(&p, bmp(40, 20, [0, 0, 255])).unwrap();
    let b = decode_image(&fs(&p), 10).unwrap();
    assert_eq!((b.width, b.height), (10, 5));
    assert_eq!(&b.rgba[..4], &[255, 0, 0, 255]);
    let full = decode_image(&fs(&p), 1600).unwrap();
    assert_eq!((full.width, full.height), (40, 20));
    std::fs::write(d.0.join("broken.png"), b"not a png").unwrap();
    assert!(decode_image(&fs(&d.0.join("broken.png")), 100).is_none());
}

#[test]
fn reads_grouped_properties_and_finds_no_handler_for_unknown_types() {
    com_sta();
    let d = temp("props");
    let p = d.0.join("notes.txt");
    std::fs::write(&p, "hello").unwrap();
    let groups = properties(&fs(&p));
    assert!(groups.iter().all(|g| !g.rows.is_empty()));
    assert!(groups.iter().flat_map(|g| &g.rows).any(|(k, v)| !k.is_empty() && !v.is_empty()));
    assert_eq!(handler(b"flit-no-such-type"), None);
}
