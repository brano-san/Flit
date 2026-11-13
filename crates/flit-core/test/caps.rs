use super::*;

#[test]
fn native_paths_follow_the_platform() {
    let windows = Caps::default();
    let linux = Caps { unix_paths: true };
    assert_eq!(windows.native_path(r#" "C:\Users\a" "#).map(|p| p.as_bytes().to_vec()), Some(br"C:\Users\a".to_vec()));
    assert!(windows.native_path(r"\\srv\share\x").is_some());
    assert!(windows.native_path("/home/a").is_none());
    assert_eq!(linux.native_path("  /home/a  ").map(|p| p.as_bytes().to_vec()), Some(b"/home/a".to_vec()));
    assert!(linux.native_path(r"C:\Users").is_none());
    for caps in [windows, linux] {
        assert!(caps.native_path("This PC").is_none());
        assert!(caps.native_path("Recycle Bin").is_none());
        assert!(caps.native_path("relative").is_none());
    }
}

#[test]
fn links_and_flatten_are_offered_on_both_platforms_but_libraries_only_on_windows() {
    let (windows, linux) = (Caps::default(), Caps { unix_paths: true });
    for id in ["file.shortcut", "file.paste_shortcut", "file.flatten"] {
        assert!(windows.supports(id) && linux.supports(id), "{id}");
    }
    for id in ["libraries.new", "libraries.restore"] {
        assert!(windows.supports(id) && !linux.supports(id), "{id}");
    }
}
