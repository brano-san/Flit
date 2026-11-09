use super::*;
use ed25519_compact::{KeyPair, Seed};
use flit_core::update::{base64_encode, render_manifest, LINUX, WINDOWS};

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> Temp {
    let d = std::env::temp_dir().join(format!("flit-update-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    Temp(d)
}

fn keys(seed: u8) -> (KeyPair, String) {
    let pair = KeyPair::from_seed(Seed::new([seed; 32]));
    let public = base64_encode(pair.pk.as_ref());
    (pair, public)
}

fn sign(pair: &KeyPair, bytes: &[u8]) -> String {
    base64_encode(pair.sk.sign(bytes, None).as_ref())
}

#[cfg(windows)]
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[cfg(not(windows))]
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, format!("flit-9.0.0-x86_64-linux/bin/{name}"), *bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn full_archive() -> Vec<u8> {
    let mut files: Vec<(&str, &[u8])> = BINARIES.iter().map(|n| (*n, b"new build".as_slice())).collect();
    files.push(("LICENSE", b"MIT"));
    archive(&files)
}

fn release(signature: String) -> Release {
    Release { version: "9.0.0".into(), notes: String::new(), url: "https://example.com/flit.zip".into(), signature }
}

fn installed(dir: &Path) -> PathBuf {
    for name in BINARIES {
        fs::write(dir.join(name), b"old build").unwrap();
    }
    dir.join(BINARIES[0])
}

fn contents(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut all: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).map(|e| (e.file_name().to_string_lossy().into_owned(), fs::read(e.path()).unwrap())).collect();
    all.sort();
    all
}

#[test]
fn signatures_verify_only_the_signed_bytes_with_the_right_key() {
    let (pair, public) = keys(7);
    let (_, other) = keys(8);
    let bytes = b"flit release archive".to_vec();
    let signature = sign(&pair, &bytes);
    assert_eq!(verify(&bytes, &signature, &public), Ok(()));
    let mut tampered = bytes.clone();
    tampered[0] ^= 1;
    assert!(verify(&tampered, &signature, &public).is_err());
    assert!(verify(&bytes, &signature, &other).is_err(), "another key");
    assert!(verify(&bytes, "not base64!", &public).is_err());
    assert!(verify(&bytes, &signature, "QUJD").is_err());
}

#[test]
fn shipped_public_key_is_a_valid_key() {
    let key = base64_decode(PUBLIC_KEY).unwrap();
    assert!(PublicKey::from_slice(&key).is_ok());
}

#[test]
fn check_with_a_fake_manifest_offers_only_newer_versions() {
    let manifest = |v: &str| render_manifest(v, "notes", "2026-11-09T00:00:00Z", &[(WINDOWS, "https://x/a.zip".into(), "QUJD".into()), (LINUX, "https://x/a.tar.gz".into(), "QUJD".into())]);
    assert_eq!(decide(manifest("0.4.0").as_bytes(), PLATFORM, "0.3.2").unwrap().map(|r| r.version), Some("0.4.0".into()));
    assert_eq!(decide(manifest("0.3.2").as_bytes(), PLATFORM, "0.3.2").unwrap(), None);
    assert_eq!(decide(manifest("0.3.1").as_bytes(), PLATFORM, "0.3.2").unwrap(), None);
    assert!(decide(b"<html>rate limited</html>", PLATFORM, "0.3.2").is_err());
}

#[test]
fn quiet_runs_never_use_the_network() {
    assert!(!network_allowed(true), "metrics runs");
}

#[test]
fn install_swaps_the_binaries_and_keeps_the_old_ones_until_next_start() {
    let t = temp("swap");
    let exe = installed(&t.0);
    let (pair, public) = keys(7);
    let bytes = full_archive();
    let (mut asked, mut installing) = (Vec::new(), 0);
    let r = install(&release(sign(&pair, &bytes)), &exe, &public, &mut |url| {
        asked.push(url.to_owned());
        Ok(bytes.clone())
    }, &mut || installing += 1);
    assert_eq!(r, Ok(()));
    assert_eq!((asked, installing), (vec!["https://example.com/flit.zip".to_owned()], 1));
    for name in BINARIES {
        assert_eq!(fs::read(t.0.join(name)).unwrap(), b"new build");
        assert_eq!(fs::read(t.0.join(format!("{name}.old"))).unwrap(), b"old build");
    }
    assert!(!t.0.join("LICENSE").exists(), "only binaries are replaced");
    remove_old(&exe);
    let names: Vec<String> = contents(&t.0).into_iter().map(|(n, _)| n).collect();
    let mut expected: Vec<String> = BINARIES.iter().map(|n| n.to_string()).collect();
    expected.sort();
    assert_eq!(names, expected);
}

#[test]
fn bad_signature_wrong_key_or_missing_binary_leave_the_install_untouched() {
    let t = temp("untouched");
    let exe = installed(&t.0);
    let before = contents(&t.0);
    let (pair, public) = keys(7);
    let (stranger, _) = keys(9);
    let bytes = full_archive();
    let mut tampered = bytes.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    let mut partial: Vec<(&str, &[u8])> = BINARIES[1..].iter().map(|n| (*n, b"x".as_slice())).collect();
    partial.push(("README.md", b"y"));
    let incomplete = archive(&partial);
    let cases = [(tampered, sign(&pair, &bytes)), (bytes.clone(), sign(&stranger, &bytes)), (incomplete.clone(), sign(&pair, &incomplete))];
    for (served, signature) in cases {
        let r = install(&release(signature), &exe, &public, &mut |_| Ok(served.clone()), &mut || {});
        assert!(matches!(r, Err(Failure::Failed(_))), "{r:?}");
        assert_eq!(contents(&t.0), before);
    }
    let r = install(&release(sign(&pair, &bytes)), &exe, &public, &mut |_| Err("offline".into()), &mut || {});
    assert_eq!(r, Err(Failure::Failed("offline".into())));
    assert_eq!(contents(&t.0), before);
}

#[test]
fn missing_folder_is_reported_as_not_writable() {
    let t = temp("readonly");
    let exe = t.0.join("gone").join(BINARIES[0]);
    let r = install(&release(String::new()), &exe, PUBLIC_KEY, &mut |_| panic!("no download into a folder Flit cannot write"), &mut || {});
    assert_eq!(r, Err(Failure::ReadOnly(t.0.join("gone").display().to_string())));
}

#[test]
fn running_exe_keeps_its_own_name() {
    let t = temp("renamed");
    let exe = t.0.join(if cfg!(windows) { "flit-dev.exe" } else { "flit-dev" });
    fs::write(&exe, b"old build").unwrap();
    let (pair, public) = keys(7);
    let bytes = full_archive();
    install(&release(sign(&pair, &bytes)), &exe, &public, &mut |_| Ok(bytes.clone()), &mut || {}).unwrap();
    assert_eq!(fs::read(&exe).unwrap(), b"new build");
    assert!(!t.0.join(BINARIES[0]).exists());
}

#[test]
fn failed_swap_rolls_back_what_was_already_replaced() {
    let t = temp("rollback");
    fs::write(t.0.join("a"), b"old a").unwrap();
    fs::write(t.0.join("b"), b"old b").unwrap();
    for k in 0..=OLD_SLOTS {
        let blocked = old_name(&t.0.join("b"), k);
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("busy"), b"").unwrap();
    }
    let r = swap(&t.0, &[("a".into(), b"new a".to_vec()), ("b".into(), b"new b".to_vec())]);
    assert!(r.is_err());
    assert_eq!(fs::read(t.0.join("a")).unwrap(), b"old a");
    assert_eq!(fs::read(t.0.join("b")).unwrap(), b"old b");
    assert!(!t.0.join("a.new").exists() && !t.0.join("b.new").exists() && !t.0.join("a.old").exists());
}

#[test]
#[ignore = "downloads from GitHub"]
fn live_release_downloads_through_redirects() {
    let name = if cfg!(windows) { "flit-0.3.2-x86_64-windows.zip" } else { "flit-0.3.2-x86_64-linux.tar.gz" };
    let mut seen = (0, None);
    let bytes = fetch::<crate::P>(&format!("https://github.com/brano-san/Flit/releases/download/v0.3.2/{name}"), &mut |r, t| {
        seen = (r, t);
        true
    })
    .unwrap();
    assert_eq!(seen, (bytes.len() as u64, Some(bytes.len() as u64)));
    assert_eq!(binaries(&bytes).unwrap().len(), BINARIES.len());
    println!("{:?}", check::<crate::P>(false));
}
