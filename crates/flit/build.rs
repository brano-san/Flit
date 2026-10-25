use std::path::Path;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const MOVEABLE_DISCARDABLE: u16 = 0x1010;
const MOVEABLE_PURE_DISCARDABLE: u16 = 0x1030;

fn res_entry(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    [0xFFFF, kind, 0xFFFF, id].iter().for_each(|v: &u16| out.extend_from_slice(&v.to_le_bytes()));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&[0; 10]);
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(4), 0);
}

fn icon_res(ico: &[u8]) -> Vec<u8> {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]) as usize;
    let count = u16_at(4);
    let mut res = Vec::new();
    res_entry(&mut res, 0, 0, 0, &[]);
    let mut group: Vec<u8> = [0u16, 1, count].iter().flat_map(|v| v.to_le_bytes()).collect();
    for id in 1..=count {
        let e = 6 + 16 * (id as usize - 1);
        let (size, offset) = (u32_at(e + 8), u32_at(e + 12));
        res_entry(&mut res, RT_ICON, id, MOVEABLE_DISCARDABLE, &ico[offset..offset + size]);
        group.extend_from_slice(&ico[e..e + 4]);
        group.extend_from_slice(&u16_at(e + 4).max(1).to_le_bytes());
        group.extend_from_slice(&ico[e + 6..e + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    res_entry(&mut res, RT_GROUP_ICON, 1, MOVEABLE_PURE_DISCARDABLE, &group);
    res
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        for dll in ["d3d11", "dcomp", "dwrite", "dwmapi", "imm32", "ole32", "oleaut32", "shell32", "combase"] {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}.dll");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
        println!("cargo:rustc-link-arg-bins=/IGNORE:4199");
        let ico = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../assets/icon/flit.ico");
        println!("cargo:rerun-if-changed={}", ico.display());
        let res = Path::new(&std::env::var("OUT_DIR").unwrap()).join("flit.res");
        std::fs::write(&res, icon_res(&std::fs::read(&ico).unwrap())).unwrap();
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
}
