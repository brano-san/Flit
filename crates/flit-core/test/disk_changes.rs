use super::*;

#[test]
fn csv_preserves_rename_events_and_escapes_text_for_spreadsheets() {
    let mut report = Report { changes: Vec::new(), truncated: false };
    for (kind, name) in [(ChangeKind::RenameFrom, "old, \"name\""), (ChangeKind::RenameTo, "=unexpected")] {
        report.changes.push(Change { at: 116_444_736_000_000_000, kind, name: name.into(), id: 7, parent: 9 });
    }
    let csv = report.csv();
    assert!(csv.starts_with("\u{feff}time_utc,"));
    assert!(csv.contains("rename_from,\"old, \"\"name\"\"\",7,9\r\n"));
    assert!(csv.contains("rename_to,\"'=unexpected\",7,9\r\n"));
}

#[test]
fn an_empty_report_has_a_header_and_no_fake_rows() {
    let report = Report { changes: Vec::new(), truncated: false };
    assert_eq!(report.csv().lines().count(), 1);
}
