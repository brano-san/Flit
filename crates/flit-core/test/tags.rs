use super::*;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn key(id: u64) -> FileKey {
    FileKey { volume: 7, id }
}

#[test]
fn store_starts_with_important_and_validates_names() {
    let mut s = TagStore::default();
    assert_eq!(s.tags, [Tag { name: "Important".into(), color: 0xE81123 }]);
    assert_eq!(s.add("  Work ", COLORS[5].1), Ok(1));
    assert_eq!(s.tags[1].name, "Work");
    for bad in ["", "   ", ".hidden", "end.", "a\"b", "tab\there", "important", &"x".repeat(65)] {
        assert!(s.add(bad, 0).is_err(), "{bad:?}");
    }
    assert!(s.add(&"ж".repeat(64), 0).is_ok());
    assert!(s.rename(1, "WORK").is_ok(), "renaming a tag to its own name in another case");
    assert_eq!(s.rename(1, "important"), Err("A tag with this name already exists"));
    assert!(s.rename(9, "Free").is_err());
    s.recolor(1, COLORS[3].1);
    assert_eq!((color_name(s.tags[1].color), color_name(0x123456)), ("Green", "Custom"));
    while s.tags.len() < MAX_TAGS {
        let n = s.tags.len();
        s.add(&format!("t{n}"), 0).unwrap();
    }
    assert_eq!(s.add("one more", 0), Err("Flit supports at most 32 tags"));
    assert_eq!(s.all(), u32::MAX);
    assert_eq!(find(&s.tags, " work "), Some(1));
}

#[test]
fn deleting_a_tag_shifts_masks_and_drops_untagged_files() {
    let mut s = TagStore::default();
    s.add("A", 0).unwrap();
    s.add("B", 0).unwrap();
    assert_eq!(s.update(key(1), p(r"C:\t\one"), |m| m | 0b010), 0b010);
    assert_eq!(s.update(key(2), p(r"C:\t\two"), |m| m | 0b101), 0b101);
    assert_eq!(s.update(key(3), p(r"C:\t\three"), |_| u32::MAX), 0b111, "masks keep only existing tags");
    assert_eq!(s.update(key(3), p(r"C:\t\three"), |m| m & !0b001), 0b110);
    assert!(s.delete(1));
    assert!(!s.delete(5));
    assert_eq!(s.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Important", "B"]);
    assert!(!s.files.contains_key(&key(1)));
    assert_eq!(s.files[&key(2)].mask, 0b11);
    assert_eq!(s.files[&key(3)].mask, 0b10);
    assert_eq!(s.update(key(3), p(r"C:\t\three"), |_| 0), 0);
    assert!(!s.files.contains_key(&key(3)));
    assert_eq!((remove_bit(0b1011, 0), remove_bit(0b1011, 2), remove_bit(1 << 31, 31), remove_bit(u32::MAX, 31)), (0b101, 0b111, 0, u32::MAX >> 1));
    assert_eq!(bits(0b1001_0010).collect::<Vec<_>>(), [1, 4, 7]);
}

#[test]
fn published_tags_answer_lookups_by_key_and_volume() {
    let mut s = TagStore::default();
    s.add("Work", 0).unwrap();
    s.update(key(1), p(r"C:\t\one"), |_| 0b11);
    s.update(FileKey { volume: 9, id: 1 }, p(r"D:\t\$Recycle.Bin\S-1\$R1"), |_| 0b10);
    s.update(key(4), p(r"C:\t\gone"), |_| 0b01);
    let mut seen = FxHashMap::default();
    seen.insert(key(1), (Meta { size: 5, ..Meta::default() }, true));
    seen.insert(key(4), (Meta::default(), false));
    let t = Tags::new(3, &s, &seen);
    assert_eq!((t.version, t.defs.len()), (3, 2));
    assert_eq!((t.mask(key(1)), t.mask(key(2)), t.mask(FileKey { volume: 9, id: 1 })), (0b11, 0, 0b10));
    assert!(t.any() && t.has_volume(7) && t.has_volume(9) && !t.has_volume(8));
    assert_eq!((t.count(0), t.count(1)), (2, 2));
    let present = |s: &str| t.files.iter().find(|f| f.path.display() == s).map(|f| (f.present, f.meta.size));
    assert_eq!(present(r"C:\t\one"), Some((true, 5)));
    assert_eq!(present(r"C:\t\gone"), Some((false, 0)));
    assert_eq!(present(r"D:\t\$Recycle.Bin\S-1\$R1"), Some((false, 0)), "deleted files are not offered");
    assert!(!Tags::default().any());
    let cell = TagCell::default();
    let shared = Arc::new(t);
    cell.clone().set(shared.clone());
    assert!(Arc::ptr_eq(&cell.get(), &shared));
}

#[test]
fn tag_queries_quote_names_and_are_detected() {
    assert_eq!(query_for("Work"), "tag:Work");
    assert_eq!(query_for("Long name"), "tag:\"Long name\"");
    for yes in ["tag:x", "report TAG:", "a !tag:\"b c\"", "  Tag:Work  "] {
        assert!(has_tag_term(yes), "{yes}");
    }
    for no in ["", "tags", "\"a tag:x\"", "notag:x", "ta"] {
        assert!(!has_tag_term(no), "{no}");
    }
}
