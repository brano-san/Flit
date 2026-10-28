use super::*;

fn row(label: &str) -> Row {
    Row { label: label.to_owned(), path: String::new(), kind: Kind::Typed(label.to_owned()) }
}

#[test]
fn the_field_text_of_a_folder_ends_with_its_separator() {
    assert_eq!(with_sep(&Location::parse(r"C:\x").unwrap()), r"C:\x\");
    assert_eq!(with_sep(&Location::parse(r"C:\").unwrap()), r"C:\");
    assert_eq!(with_sep(&Location::computer()), "", "This PC starts with an empty field");
}

#[test]
fn rows_rank_by_fuzzy_score_and_keep_listing_order_without_a_query() {
    let rows = [row("beta"), row("alpha"), row("Alphabet"), row("gamma")];
    let all = ranked("", &rows, 0);
    assert_eq!(all.len(), 4);
    assert!(all.iter().enumerate().all(|(k, l)| matches!(l, Line::Row(n, m) if *n == k && m.is_empty())));
    let got: Vec<usize> = ranked("alp", &rows, 0).iter().map(|l| if let Line::Row(n, _) = l { *n } else { usize::MAX }).collect();
    assert_eq!(got, [1, 2]);
    assert!(ranked("zz", &rows, 0).is_empty());
}

#[test]
fn typed_rows_resolve_to_locations_and_know_folders() {
    let typed = row(r"C:\x\");
    assert_eq!(typed.loc(), Location::parse(r"C:\x"));
    assert!(typed.folder());
    assert!(!row(r"C:\x\f.txt").folder());
}
