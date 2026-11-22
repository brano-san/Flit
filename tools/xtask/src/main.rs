mod colors;
mod layers;

use std::error::Error;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

#[cfg(test)]
#[path = "../test/checks.rs"]
mod tests;

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).ok_or("workspace root not found")?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--all-features", "--locked"])
        .output()?;
    if !output.status.success() {
        return Err(format!("cargo metadata failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    let metadata = serde_json::from_slice(&output.stdout)?;
    let mut problems = Vec::new();
    let members = layers::check(&metadata, &mut problems);
    for member in members {
        let dir = member.manifest_path.parent().ok_or("package manifest has no parent")?;
        colors::check_dir(root, &dir.join("src"), &mut problems)?;
    }
    if !problems.is_empty() {
        for problem in problems {
            eprintln!("{problem}");
        }
        return Err("architecture checks failed".into());
    }
    println!("Workspace dependency layers, Windows isolation and theme colors passed.");
    Ok(())
}
