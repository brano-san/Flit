use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../flit-proto/src/build_id.rs"]
mod build_id;

const RT_ICON: u16 = 3;
const AGENT_SOURCES: [&str; 8] = ["crates/flit-platform-linux", "crates/flit-proto", "crates/flit-core", "crates/flit-platform", "crates/flit-search", "Cargo.toml", "Cargo.lock", ".cargo/config.toml"];
const AGENTS: [(&str, &str, &str); 2] = [("x64", "FLIT_WSL_AGENT_X64", "x86_64-unknown-linux-musl"), ("arm64", "FLIT_WSL_AGENT_ARM64", "aarch64-unknown-linux-musl")];
const OUTER_BUILD_ENV: [&str; 6] = ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET"];
const RT_GROUP_ICON: u16 = 14;
const MOVEABLE_DISCARDABLE: u16 = 0x1010;
const MOVEABLE_PURE_DISCARDABLE: u16 = 0x1030;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Flit.FileManager" version="VERSION" processorArchitecture="amd64"/>
  <description>Flit file manager</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security></trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application></compatibility>
</assembly>
"#;

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

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

fn version_node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0; 2];
    b.extend_from_slice(&value_len.to_le_bytes());
    b.extend_from_slice(&(text as u16).to_le_bytes());
    b.extend(utf16(key));
    b.resize(b.len().next_multiple_of(4), 0);
    b.extend_from_slice(value);
    for c in children {
        b.resize(b.len().next_multiple_of(4), 0);
        b.extend_from_slice(c);
    }
    let len = b.len() as u16;
    b[..2].copy_from_slice(&len.to_le_bytes());
    b
}

fn version_res(res: &mut Vec<u8>) {
    let part = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let (major, minor, patch) = (part("CARGO_PKG_VERSION_MAJOR"), part("CARGO_PKG_VERSION_MINOR"), part("CARGO_PKG_VERSION_PATCH"));
    let version = format!("{major}.{minor}.{patch}.0");
    let fixed: Vec<u8> = [0xFEEF_04BD, 0x0001_0000, major << 16 | minor, patch << 16, major << 16 | minor, patch << 16, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0].iter().flat_map(|v: &u32| v.to_le_bytes()).collect();
    let strings: Vec<Vec<u8>> = [("CompanyName", "Flit"), ("FileDescription", "Flit file manager"), ("FileVersion", version.as_str()), ("InternalName", "flit"), ("LegalCopyright", "Copyright (c) 2026 Andrey, MIT License"), ("OriginalFilename", "flit.exe"), ("ProductName", "Flit"), ("ProductVersion", version.as_str())]
        .iter()
        .map(|(k, v)| version_node(k, &utf16(v), v.encode_utf16().count() as u16 + 1, true, &[]))
        .collect();
    let table = version_node("040904B0", &[], 0, true, &strings);
    let string_info = version_node("StringFileInfo", &[], 0, true, &[table]);
    let translation = version_node("Translation", &[0x09, 0x04, 0xB0, 0x04], 4, false, &[]);
    let var_info = version_node("VarFileInfo", &[], 0, true, &[translation]);
    let root = version_node("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[string_info, var_info]);
    res_entry(res, RT_VERSION, 1, MOVEABLE_PURE_DISCARDABLE, &root);
    res_entry(res, RT_MANIFEST, 1, MOVEABLE_PURE_DISCARDABLE, MANIFEST.replace("VERSION", &version).as_bytes());
}

fn build_agent(root: &Path, target: &str) -> Option<PathBuf> {
    AGENT_SOURCES.iter().for_each(|s| println!("cargo:rerun-if-changed={}", root.join(s).display()));
    let dir = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from).join("wsl-agent");
    let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo.current_dir(root).args(["build", "--quiet", "--locked", "-p", "flit-platform-linux", "--bin", "flit-wsl-agent", "--features", "agent", "--profile", "wsl-agent", "--target", target, "--target-dir"]).arg(&dir);
    for v in OUTER_BUILD_ENV {
        cargo.env_remove(v);
    }
    match cargo.status() {
        Ok(s) if s.success() => Some(dir.join(target).join("wsl-agent").join("flit-wsl-agent")),
        other => {
            println!("cargo:warning=the WSL agent for {target} was not built ({other:?}), so WSL folders will open through \\\\wsl.localhost");
            None
        }
    }
}

fn embed_agents() {
    let root = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let mut code = String::from("pub const AGENTS: flit_platform::AgentImages = flit_platform::AgentImages {\n");
    for (arch, var, target) in AGENTS {
        println!("cargo:rerun-if-env-changed={var}");
        let given = std::env::var_os(var).map(PathBuf::from);
        given.iter().for_each(|p| println!("cargo:rerun-if-changed={}", p.display()));
        let path = given.or_else(|| (windows && arch == "x64").then(|| build_agent(&root, target)).flatten());
        match path.and_then(|p| Some((std::fs::read(&p).ok()?, p))) {
            Some((bytes, p)) => code += &format!("    {arch}: Some(flit_platform::AgentImage {{ bytes: include_bytes!({:?}), build: {:?} }}),\n", p.display().to_string(), build_id::build_id(&bytes)),
            None => code += &format!("    {arch}: None,\n"),
        }
    }
    code += "};\n";
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("wsl_agent.rs"), code).unwrap();
}

fn main() {
    embed_agents();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        for dll in ["d3d11", "dcomp", "dwrite", "dwmapi", "imm32", "ole32", "oleaut32", "shell32", "combase"] {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}.dll");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
        println!("cargo:rustc-link-arg-bins=/IGNORE:4199");
        let ico = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../assets/icon/flit.ico");
        println!("cargo:rerun-if-changed={}", ico.display());
        let res = Path::new(&std::env::var("OUT_DIR").unwrap()).join("flit.res");
        let mut resources = icon_res(&std::fs::read(&ico).unwrap());
        version_res(&mut resources);
        std::fs::write(&res, resources).unwrap();
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
}
