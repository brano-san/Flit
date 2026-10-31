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

#[test]
fn the_address_window_shows_the_system_folder_icon_in_its_field() {
    let mut a = crate::ui::harness::app(&["a.txt"]);
    a.open_address();
    let here = a.current_location().to_windows().unwrap();
    let Some(Some((key, kind, path))) = a.go_field_icon(a.nav.palette.as_ref().unwrap()) else { panic!("no folder icon for the field") };
    assert_eq!((kind, path), (IconKind::Folder, here));
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    g.add_image(key, 16, 16, vec![0xFF; 16 * 16 * 4]);
    a.frame(&mut g, Instant::now());
    let field = a.palette_field().unwrap();
    let lead = Rect::new(field.x, field.y, field.h, field.h);
    assert!(g.dl.instances.iter().any(|i| i.kind == crate::render::kind::IMAGE && lead.contains(i.rect[0] + i.rect[2] / 2.0, i.rect[1] + i.rect[3] / 2.0)), "the field draws the folder's own icon");
}

#[test]
fn a_typed_folder_row_asks_for_the_folder_icon() {
    let a = crate::ui::harness::app(&["a.txt"]);
    let row = Row { label: r"D:\Games\".into(), path: String::new(), kind: Kind::Typed(r"D:\Games\".into()) };
    assert!(row.folder());
    assert!(Location::parse(r"D:\Games\").and_then(|l| folder_icon(&l, &a.nav.sidebar, 16)).is_some_and(|(_, kind, _)| kind == IconKind::Folder));
}
