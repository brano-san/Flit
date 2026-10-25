use super::*;

#[test]
fn layout_and_colors() {
    assert_eq!(std::mem::size_of::<Instance>(), 64);
    assert_eq!(Color::rgb(0x112233).0.to_le_bytes(), [0x11, 0x22, 0x33, 0xFF]);
    assert_eq!(Color::rgb(0x112233).alpha(0x80).0.to_le_bytes(), [0x11, 0x22, 0x33, 0x80]);
    assert_eq!(Rect::new(0.4, 0.6, 9.2, 9.3).snap(), Rect::new(0.0, 1.0, 10.0, 9.0));
}
