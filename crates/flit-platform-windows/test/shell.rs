use super::*;
use flit_core::provider::FileOp;
use flit_core::ui::IconRequest;
use flit_platform::FileOps;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

#[test]
fn shortcuts_resolve_only_to_existing_folders() {
    let root = std::env::temp_dir().join(format!("flit-shell-lnk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("f.txt"), "x").unwrap();
    let at = |rel: &str| p(root.join(rel).to_str().unwrap());
    let mut sh = ShellCom::new().unwrap();
    let out = sh.file_op(&FileOp::Link { items: vec![at("target"), at("f.txt")], dest: at("") });
    let [(_, to_folder), (_, to_file)] = out.items.try_into().unwrap();
    assert_eq!(sh.shortcut_folder(&to_folder), Some(at("target")));
    assert_eq!(sh.shortcut_folder(&to_file), None);
    assert_eq!(sh.shortcut_folder(&at("f.txt")), None);
    std::fs::remove_dir(root.join("target")).unwrap();
    assert_eq!(sh.shortcut_folder(&to_folder), None, "a missing target is left to the shell");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn terminal_commands_open_one_tab_per_folder_with_quoted_paths() {
    let dirs = vec![p(r"C:\a b"), p(r"C:\"), p(r"D:\x;y"), p(r"\\wsl.localhost\Ubuntu\home\u")];
    let tabs = r#"new-tab -d "C:\a b" ; new-tab -d C:\ ; new-tab -d D:\x\;y ; new-tab wsl.exe -d Ubuntu --cd /home/u"#;
    let wt = commands(&Launch::Terminal { dirs: dirs.clone(), admin: true }, Some(r"C:\wt.exe"));
    assert_eq!(wt, [Command { program: r"C:\wt.exe".into(), args: tabs.into(), dir: None, admin: true }]);
    let consoles = commands(&Launch::Terminal { dirs, admin: false }, None);
    let shown: Vec<(&str, &str)> = consoles.iter().map(|c| (c.program.as_str(), c.args.as_str())).collect();
    assert_eq!(shown, [("cmd.exe", r#"/k pushd "C:\a b""#), ("cmd.exe", r#"/k pushd "C:\""#), ("cmd.exe", r#"/k pushd "D:\x;y""#), ("wsl.exe", "-d Ubuntu --cd /home/u")]);
    assert!(consoles.iter().all(|c| !c.admin && c.dir.is_none()));
}

#[test]
fn dropped_files_become_quoted_program_arguments() {
    let launch = Launch::With { program: p(r"C:\tools\run.cmd"), args: vec![p(r"D:\in\a b.txt"), p(r"D:\in\c.txt"), p(r"E:\")] };
    let expected = Command { program: r"C:\tools\run.cmd".into(), args: r#""D:\in\a b.txt" D:\in\c.txt E:\"#.into(), dir: Some(r"C:\tools".into()), admin: false };
    assert_eq!(commands(&launch, Some(r"C:\wt.exe")), [expected]);
    assert_eq!(quote_path(r"C:\My Folder\"), r#""C:\My Folder\\""#);
    assert_eq!(quote_path(""), r#""""#);
}

fn solid(size: u32, px: [u8; 4]) -> Bitmap {
    Bitmap { width: size, height: size, rgba: px.repeat((size * size) as usize) }
}

#[test]
fn fit_never_exceeds_the_requested_size() {
    let b = fit(solid(256, [10, 20, 30, 40]), 56);
    assert_eq!((b.width, b.height, b.rgba.len()), (56, 56, 56 * 56 * 4));
    assert!(b.rgba.chunks_exact(4).all(|p| p == [10, 20, 30, 40]));
    let small = fit(solid(48, [1, 2, 3, 4]), 56);
    assert_eq!((small.width, small.height), (48, 48));
}

#[test]
fn fit_box_filters_premultiplied_pixels() {
    let mut b = solid(4, [0, 0, 0, 0]);
    for y in 0..2 {
        for x in 0..2 {
            let p = ((y * 4 + x) * 4) as usize;
            b.rgba[p..p + 4].copy_from_slice(&[200, 100, 0, 200]);
        }
    }
    let half = fit(b, 2);
    assert_eq!(&half.rgba[..4], &[200, 100, 0, 200]);
    assert_eq!(&half.rgba[4..8], &[0, 0, 0, 0]);
    let quarter = fit(half, 1);
    assert_eq!(quarter.rgba, vec![50, 25, 0, 50]);
}

#[test]
fn shell_icons_match_requested_size() {
    let mut shell = ShellCom::new().unwrap();
    for size in [16, 56, 100, 300] {
        let req = IconRequest { key: 0, kind: IconKind::Type, path: FsPath::parse(r"C:\x.txt").unwrap(), size, gen: 0, row: 0 };
        let b = shell.icon(&req).expect("type icon");
        assert!(b.width <= size && b.height <= size, "{size}: {}x{}", b.width, b.height);
        assert_eq!(b.rgba.len(), (b.width * b.height * 4) as usize);
    }
}
