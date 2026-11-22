use super::*;

fn record(name: &[u8], time: i64, reason: u32) -> crate::mft::UsnRecord<'_> {
    crate::mft::UsnRecord { frn: 36, parent: 22, usn: 16, time, reason, attrs: 0, name }
}

#[test]
fn rows_filter_period_name_and_kind_and_keep_both_halves_of_a_rename() {
    let filter = Filter { since: 10, until: 20, name: "phOTO".into(), created: false, deleted: true, renamed: true };
    let mut rows = Rows::new(&filter);
    let bytes: Vec<_> = "Photo.JPG".encode_utf16().flat_map(u16::to_le_bytes).collect();
    let other: Vec<_> = "other.JPG".encode_utf16().flat_map(u16::to_le_bytes).collect();
    rows.push(&record(&bytes, 9, DELETE | CLOSE));
    rows.push(&record(&other, 15, DELETE | CLOSE));
    rows.push(&record(&bytes, 11, CREATE));
    rows.push(&record(&bytes, 13, OLD_NAME));
    rows.push(&record(&bytes, 14, NEW_NAME));
    rows.push(&record(&bytes, 20, DELETE));
    let report = rows.finish();
    let kinds = report.changes.iter().map(|r| r.kind).collect::<Vec<_>>();
    assert_eq!(kinds, vec![ChangeKind::Deleted, ChangeKind::RenameTo, ChangeKind::RenameFrom]);
    assert!(!report.truncated);
}

#[test]
fn malformed_and_out_of_period_rows_are_ignored_without_reading_a_volume() {
    let filter = Filter { since: 10, until: 20, name: String::new(), created: true, deleted: true, renamed: true };
    let mut rows = Rows::new(&filter);
    rows.push(&record(&[0x41], 13, CREATE));
    rows.push(&record(&[0x41, 0], 21, DELETE));
    rows.push(&record(&[], 13, NEW_NAME));
    assert!(rows.finish().changes.is_empty());
}

#[test]
fn a_cumulative_close_record_does_not_repeat_the_original_create() {
    let filter = Filter { since: 10, until: 20, name: String::new(), created: true, deleted: true, renamed: true };
    let mut rows = Rows::new(&filter);
    let name = [b'f', 0];
    rows.push(&record(&name, 11, CREATE));
    rows.push(&record(&name, 13, CREATE | OLD_NAME));
    rows.push(&record(&name, 14, CREATE | OLD_NAME | NEW_NAME));
    rows.push(&record(&name, 19, CREATE | OLD_NAME | NEW_NAME | CLOSE));
    let kinds: Vec<_> = rows.finish().changes.into_iter().map(|r| r.kind).collect();
    assert_eq!(kinds, [ChangeKind::RenameTo, ChangeKind::RenameFrom, ChangeKind::Created]);
}
