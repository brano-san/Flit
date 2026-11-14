use super::*;

fn class(ext: &str, progid: Option<&str>) -> (String, Option<String>) {
    (ext.to_owned(), progid.map(str::to_owned))
}

#[test]
fn extensions_whose_file_type_says_never_show_are_hidden() {
    let classes = [class(".lnk", Some("lnkfile")), class(".URL", Some("InternetShortcut")), class(".pif", Some("piffile")), class(".txt", Some("txtfile")), class(".library-ms", None)];
    let flagged = |key: &str| ["lnkfile", "InternetShortcut", "piffile", ".library-ms"].contains(&key);
    assert_eq!(hidden_by(&classes, flagged), ["library-ms", "lnk", "pif", "url"]);
}

#[test]
fn ordinary_types_and_odd_keys_keep_their_extension() {
    let classes = [class(".txt", Some("txtfile")), class(".", Some("lnkfile")), class(".none", Some(""))];
    assert!(hidden_by(&classes, |key| key == "lnkfile" || key.is_empty()).is_empty());
    assert!(hidden_by(&[], |_| true).is_empty());
}
