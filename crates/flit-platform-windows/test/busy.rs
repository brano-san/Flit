use super::*;
use std::os::windows::fs::OpenOptionsExt;

#[test]
fn a_file_held_open_names_the_program_that_holds_it_and_a_free_file_names_none() {
    let dir = crate::test_temp().join(format!("flit-busy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (held, free) = (dir.join("held.txt"), dir.join("free.txt"));
    std::fs::write(&held, "h").unwrap();
    std::fs::write(&free, "f").unwrap();
    let path = |p: &std::path::Path| FsPath::parse(p.to_str().unwrap()).unwrap();
    let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&held).unwrap();
    let names = holders(&path(&held));
    drop(lock);
    let after = holders(&path(&held));
    let none = holders(&path(&free));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(names.len() == 1 && !names[0].is_empty(), "{names:?}");
    assert!(after.is_empty() && none.is_empty(), "{after:?} {none:?}");
}
