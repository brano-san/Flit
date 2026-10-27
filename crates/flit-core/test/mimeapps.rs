use super::*;

const LIST: &str = "[Added Associations]\ninode/directory=org.gnome.Nautilus.desktop;\n\n[Default Applications]\ntext/plain=gedit.desktop\ninode/directory = org.gnome.Nautilus.desktop\n\n[Removed Associations]\n";

#[test]
fn reads_only_the_default_applications_group() {
    assert_eq!(default_app(LIST, FOLDER).as_deref(), Some("org.gnome.Nautilus.desktop"));
    assert_eq!(default_app("[Added Associations]\ninode/directory=x.desktop\n", FOLDER), None);
}

#[test]
fn setting_replaces_inserts_or_creates_the_entry() {
    let set = set_default_app(LIST, FOLDER, Some(FLIT_DESKTOP));
    assert_eq!(default_app(&set, FOLDER).as_deref(), Some(FLIT_DESKTOP));
    assert_eq!(set.replace("inode/directory=flit.desktop", "inode/directory = org.gnome.Nautilus.desktop"), LIST);
    let added = set_default_app("[Default Applications]\ntext/plain=gedit.desktop\n\n[Other]\n", FOLDER, Some(FLIT_DESKTOP));
    assert_eq!(added, "[Default Applications]\ntext/plain=gedit.desktop\ninode/directory=flit.desktop\n\n[Other]\n");
    assert_eq!(set_default_app("[Other]\na=b\n", FOLDER, Some(FLIT_DESKTOP)), "[Other]\na=b\n\n[Default Applications]\ninode/directory=flit.desktop\n");
    assert_eq!(set_default_app("", FOLDER, Some(FLIT_DESKTOP)), "[Default Applications]\ninode/directory=flit.desktop\n");
}

#[test]
fn turning_off_restores_the_file_exactly() {
    for original in [Some(LIST), Some("[Other]\na=b"), Some(""), None] {
        let saved = backup(original);
        let written = ours(original);
        for mime in FOLDER_TYPES {
            assert_eq!(default_app(&written, mime).as_deref(), Some(FLIT_DESKTOP), "{mime}");
        }
        assert_eq!(restore(&saved, Some(&written)).as_deref(), original, "{original:?}");
    }
}

#[test]
fn later_edits_by_other_programs_survive_turning_off() {
    let saved = backup(Some(LIST));
    let edited = ours(Some(LIST)).replace("text/plain=gedit.desktop", "text/plain=kate.desktop");
    let back = restore(&saved, Some(&edited)).unwrap();
    assert_eq!(default_app(&back, FOLDER).as_deref(), Some("org.gnome.Nautilus.desktop"));
    assert!(back.contains("text/plain=kate.desktop"));
    let fresh = restore(&backup(None), Some("[Default Applications]\ninode/directory=flit.desktop\nimage/png=eog.desktop\n")).unwrap();
    assert_eq!(fresh, "[Default Applications]\nimage/png=eog.desktop\n");
    assert_eq!(restore("garbage", Some("x")).as_deref(), Some("x"));
}

#[test]
fn both_folder_types_go_back_to_their_own_previous_handlers() {
    let original = "[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop\nx-directory/normal=nemo.desktop\n";
    let edited = format!("{}image/png=eog.desktop\n", ours(Some(original)));
    let back = restore(&backup(Some(original)), Some(&edited)).unwrap();
    assert_eq!(default_app(&back, "inode/directory").as_deref(), Some("org.gnome.Nautilus.desktop"));
    assert_eq!(default_app(&back, "x-directory/normal").as_deref(), Some("nemo.desktop"));
    let without = restore(&backup(Some("[Default Applications]\ninode/directory=a.desktop\n")), Some(&format!("{}b=c\n", ours(Some("[Default Applications]\ninode/directory=a.desktop\n"))))).unwrap();
    assert_eq!(default_app(&without, "x-directory/normal"), None);
}
