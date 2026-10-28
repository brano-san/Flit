use super::*;
use flit_core::location::Location;

#[test]
fn saved_layouts_round_trip_through_their_named_files_and_list_sorted() {
    let dir = std::env::temp_dir().join(format!("flit-layouts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(list(&dir).is_empty());
    let layout = SavedLayout { tabs: vec![Location::home_page()], cur: 0, panels: None, quick_access: false, quick_width: 240.0 };
    save(&dir, "work", &layout).unwrap();
    save(&dir, "Archive", &layout).unwrap();
    assert_eq!(list(&dir), ["Archive", "work"]);
    assert_eq!(load(&dir, "work"), Some(layout));
    delete(&dir, "work").unwrap();
    assert_eq!(list(&dir), ["Archive"]);
    assert_eq!(load(&dir, "work"), None);
    let _ = std::fs::remove_dir_all(&dir);
}
