use super::*;
use std::path::PathBuf;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("flit-menu-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, rel: &str) -> FsPath {
        FsPath::parse(self.0.join(rel).to_str().unwrap()).unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn verbs(m: &ShellMenu) -> Vec<String> {
    crate::com_sta();
    unsafe {
        let menu = build(HWND::default(), m).unwrap();
        let hmenu = CreatePopupMenu().unwrap();
        menu.QueryContextMenu(hmenu, 0, FIRST_ID, LAST_ID, CMF_NORMAL | CMF_CANRENAME).ok().unwrap();
        let ids: Vec<u32> = (0..GetMenuItemCount(Some(hmenu))).map(|i| GetMenuItemID(hmenu, i)).filter(|&id| id != u32::MAX && id >= FIRST_ID).collect();
        let out = ids.iter().filter_map(|&id| verb(&menu, (id - FIRST_ID) as usize)).map(|v| v.to_ascii_lowercase()).collect();
        let _ = DestroyMenu(hmenu);
        out
    }
}

#[test]
fn item_and_background_menus_offer_the_verbs_flit_takes_over() {
    let t = TempDir::new("verbs");
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    let items = verbs(&ShellMenu { dir: t.path(""), items: vec![t.path("a.txt")], x: 0.0, y: 0.0, extended: false });
    for v in ["copy", "cut", "delete", "rename", "properties"] {
        assert!(items.iter().any(|i| i == v), "{v} missing from {items:?}");
    }
    let background = verbs(&ShellMenu { dir: t.path(""), items: Vec::new(), x: 0.0, y: 0.0, extended: false });
    assert!(background.iter().any(|v| v == "properties"), "{background:?}");
    assert!(SHELL_VERBS.iter().all(|v| v.chars().all(|c| c.is_ascii_lowercase())));
}

#[test]
fn shell_data_objects_carry_the_dragged_paths() {
    crate::com_sta();
    let t = TempDir::new("data");
    for n in ["one.txt", "two words.txt"] {
        std::fs::write(t.0.join(n), n).unwrap();
    }
    let paths = vec![t.path("one.txt"), t.path("two words.txt")];
    let data: IDataObject = Pidls::parse(&paths).unwrap().ui_object(HWND::default()).unwrap();
    assert_eq!(crate::dragdrop::data_paths(&data), paths);
    assert!(Pidls::parse(&[t.path("missing.txt")]).is_err());
}
