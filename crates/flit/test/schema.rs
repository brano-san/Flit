use super::*;

pub struct Dir(pub PathBuf);

impl Dir {
    pub fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("flit-schema-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn encode(v: u32, text: &str) -> Vec<u8> {
    postcard::to_stdvec(&(v, text)).unwrap()
}

fn decode(b: &[u8]) -> Option<(u32, String)> {
    postcard::from_bytes(b).ok()
}

fn v1_to_v2(b: Vec<u8>) -> Option<Vec<u8>> {
    let (_, t) = decode(&b)?;
    Some(encode(2, &format!("{t}+2")))
}

fn v2_to_v3(b: Vec<u8>) -> Option<Vec<u8>> {
    let (_, t) = decode(&b)?;
    Some(encode(3, &format!("{t}+3")))
}

const STEPS: [Step; 2] = [v1_to_v2, v2_to_v3];

#[test]
fn a_missing_file_loads_nothing_and_leaves_nothing() {
    let d = Dir::new("missing");
    assert_eq!(load(&d.0.join("x.bin"), 3, &STEPS, decode), None);
    assert_eq!(std::fs::read_dir(&d.0).unwrap().count(), 0);
}

#[test]
fn older_files_migrate_step_by_step() {
    let d = Dir::new("migrate");
    let f = d.0.join("x.bin");
    for (v, want) in [(1, "a+2+3"), (2, "a+3"), (3, "a")] {
        std::fs::write(&f, encode(v, "a")).unwrap();
        assert_eq!(load(&f, 3, &STEPS, decode), Some((3, want.to_owned())));
        assert!(f.exists());
    }
}

#[test]
fn newer_files_are_set_aside_not_overwritten() {
    let d = Dir::new("newer");
    let f = d.0.join("x.bin");
    let bytes = encode(4, "future");
    std::fs::write(&f, &bytes).unwrap();
    assert_eq!(load(&f, 3, &STEPS, decode), None);
    assert!(!f.exists());
    assert_eq!(std::fs::read(aside(&f, NEWER)).unwrap(), bytes);
}

#[test]
fn unreadable_or_too_old_files_go_to_bad() {
    let d = Dir::new("bad");
    let f = d.0.join("x.bin");
    for bytes in [vec![0xFF, 0xFF, 0xFF], encode(0, "ancient"), vec![3, 0xFF], encode(3 + PLAUSIBLE_NEWER + 1, "garbage")] {
        std::fs::write(&f, &bytes).unwrap();
        assert_eq!(load(&f, 3, &STEPS, decode), None);
        assert_eq!(std::fs::read(aside(&f, BAD)).unwrap(), bytes);
    }
}
