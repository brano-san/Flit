use super::*;

#[test]
fn ime_cursor_positions_become_byte_offsets() {
    let units: Vec<u16> = "a日😀b".encode_utf16().collect();
    assert_eq!(preedit_from_utf16(&units, 0), Preedit::new("a日😀b".into(), Some(0)));
    assert_eq!(preedit_from_utf16(&units, 2).cursor, Some("a日".len()));
    assert_eq!(preedit_from_utf16(&units, 4).cursor, Some("a日😀".len()));
    assert_eq!(preedit_from_utf16(&units, 5).cursor, Some("a日😀b".len()));
    assert_eq!(preedit_from_utf16(&units, -1).cursor, None);
    assert_eq!(preedit_from_utf16(&units, 9).cursor, None);
    assert_eq!(preedit_from_utf16(&units, 3).cursor, None, "a cursor between surrogates is not a character boundary");
}