use super::*;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

#[test]
fn formats_come_from_extensions() {
    assert_eq!(browsable(b"a.ZIP"), Some(Format::Zip));
    assert_eq!(browsable(b"a.tar.gz"), Some(Format::TarGz));
    assert_eq!(browsable(b"a.tgz"), Some(Format::TarGz));
    assert_eq!(browsable(b"a.gz"), Some(Format::Gz));
    assert_eq!(browsable(b"a.jar"), Some(Format::Zip));
    assert_eq!(browsable(b"a.lzh"), Some(Format::Lzh));
    assert_eq!(browsable(b"a.docx"), None);
    assert_eq!(format_of(b"a.jar"), Some(Format::Zip));
    assert_eq!(format_of(b"a.msix"), Some(Format::Zip));
    assert_eq!(format_of(b"a.txt"), None);
    assert_eq!(stem(b"a.tar.gz"), b"a");
    assert_eq!(stem(b"photos.2024.zip"), b"photos.2024");
    assert_eq!(stem(b".zip"), b".zip");
}

#[test]
fn paths_split_at_every_archive_component() {
    assert_eq!(split(&p(r"C:\d\a.zip")), Some((p(r"C:\d\a.zip"), b"".to_vec())));
    assert_eq!(split(&p(r"C:\d\a.zip\x\y.txt")), Some((p(r"C:\d\a.zip"), b"x/y.txt".to_vec())));
    let all = splits(&p(r"\\srv\share\a.7z\b.zip\c"));
    assert_eq!(all, vec![(p(r"\\srv\share\a.7z"), b"b.zip/c".to_vec()), (p(r"\\srv\share\a.7z\b.zip"), b"c".to_vec())]);
    assert_eq!(split(&p("/home/u/a.tar.gz/x")), Some((p("/home/u/a.tar.gz"), b"x".to_vec())));
    assert_eq!(split(&p(r"C:\d\x")), None);
    assert_eq!(split(&FsPath::computer()), None);
    assert_eq!(split(&FsPath::recycle_bin()), None);
}

#[test]
fn remembered_choices_are_per_archive_and_case_blind() {
    let a = p(r"C:\remembered\A.zip");
    choose_encoding(&a, Some(Encoding::Cp866));
    set_password(&a, Some(Secret("pw".into())));
    let same = p(r"c:\REMEMBERED\a.zip");
    assert_eq!(chosen_encoding(&same), Some(Encoding::Cp866));
    assert_eq!(password(&same), Some(Secret("pw".into())));
    assert_eq!(chosen_encoding(&p(r"C:\remembered\b.zip")), None);
    assert_eq!(format!("{:?}", Secret("pw".into())), "Secret");
}

#[test]
fn operations_map_their_paths() {
    let op = ArchiveOp::Extract { archives: vec![p(r"C:\a.zip")], dest: Some(p(r"C:\out")), place: Place::Smart };
    let moved = op.map_paths(|x| FsPath::parse(&x.display().replace("C:", "D:")));
    assert_eq!(moved, Some(ArchiveOp::Extract { archives: vec![p(r"D:\a.zip")], dest: Some(p(r"D:\out")), place: Place::Smart }));
}
