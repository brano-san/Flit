use super::*;

#[test]
fn fonts_are_copied_under_their_own_name_and_other_files_are_refused() {
    let dir = std::env::temp_dir().join(format!("flit-fonts-{}", std::process::id()));
    let src = dir.join("in");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("Demo Sans.otf"), b"OTTO").unwrap();
    std::fs::write(src.join("notes.txt"), b"x").unwrap();
    let path = |name: &str| FsPath::parse(&src.join(name).to_string_lossy()).unwrap();
    let out = dir.join("fonts");
    let (to, value) = copy_font(&path("Demo Sans.otf"), &out).unwrap();
    assert_eq!((to, value.as_str()), (out.join("Demo Sans.otf"), "Demo Sans (OpenType)"));
    assert_eq!(std::fs::read(out.join("Demo Sans.otf")).unwrap(), b"OTTO");
    assert!(copy_font(&path("notes.txt"), &out).unwrap_err().message.contains("notes.txt"));
    assert!(!out.join("notes.txt").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
