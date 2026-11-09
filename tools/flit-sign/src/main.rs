use ed25519_compact::{KeyPair, Seed};
use flit_core::update::{base64_decode, base64_encode, platform_of_asset, render_manifest};
use std::path::Path;
use std::process::ExitCode;

const KEY_VAR: &str = "FLIT_UPDATE_KEY";
const USAGE: &str = "usage:
  flit-sign keygen
  flit-sign sign <file>
  flit-sign manifest <version> <pub-date> <base-url> <notes-file> <archive>...
sign and manifest read the base64 secret key from FLIT_UPDATE_KEY";

#[cfg(windows)]
fn entropy(buf: &mut [u8]) -> Result<(), String> {
    use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    unsafe { BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok().map_err(|e| format!("BCryptGenRandom: {e}"))
}

#[cfg(not(windows))]
fn entropy(buf: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(buf)).map_err(|e| format!("/dev/urandom: {e}"))
}

fn keygen() -> Result<(), String> {
    let mut seed = [0u8; Seed::BYTES];
    entropy(&mut seed)?;
    let pair = KeyPair::from_seed(Seed::new(seed));
    println!("public: {}", base64_encode(pair.pk.as_ref()));
    println!("secret: {}", base64_encode(&seed));
    Ok(())
}

fn key() -> Result<KeyPair, String> {
    let text = std::env::var(KEY_VAR).map_err(|_| format!("{KEY_VAR} is not set"))?;
    let seed = base64_decode(&text).ok_or_else(|| format!("{KEY_VAR} is not base64"))?;
    let seed = Seed::from_slice(&seed).map_err(|_| format!("{KEY_VAR} must hold {} bytes", Seed::BYTES))?;
    Ok(KeyPair::from_seed(seed))
}

fn sign(pair: &KeyPair, file: &Path) -> Result<String, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let signature = pair.sk.sign(&bytes, None);
    pair.pk.verify(&bytes, &signature).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(base64_encode(signature.as_ref()))
}

fn manifest(version: &str, date: &str, base: &str, notes: &Path, archives: &[&str]) -> Result<(), String> {
    let pair = key()?;
    let notes = std::fs::read_to_string(notes).map_err(|e| format!("{}: {e}", notes.display()))?;
    let mut platforms = Vec::new();
    for archive in archives {
        let path = Path::new(archive);
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("{archive}: no file name"))?;
        let platform = platform_of_asset(name).ok_or_else(|| format!("{name}: not a Flit release archive"))?;
        platforms.push((platform, format!("{}/{name}", base.trim_end_matches('/')), sign(&pair, path)?));
    }
    print!("{}", render_manifest(version, notes.trim(), date, &platforms));
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["keygen"] => keygen(),
        ["sign", file] => key().and_then(|pair| sign(&pair, Path::new(file))).map(|s| println!("{s}")),
        ["manifest", version, date, base, notes, archives @ ..] if !archives.is_empty() => manifest(version, date, base, Path::new(notes), archives),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flit-sign: {e}");
            ExitCode::FAILURE
        }
    }
}
