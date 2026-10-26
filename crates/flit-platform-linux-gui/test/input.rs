use super::*;

fn chars(c: &str) -> WinitKey {
    WinitKey::Character(c.into())
}

#[test]
fn cyrillic_layout_maps_letters_by_us_position() {
    assert_eq!(key(PhysicalKey::Code(KeyCode::KeyC), &chars("с")), Key::Char('C'));
    assert_eq!(key(PhysicalKey::Code(KeyCode::KeyV), &chars("м")), Key::Char('V'));
    assert_eq!(key(PhysicalKey::Code(KeyCode::Period), &chars("ю")), Key::Char('.'));
    assert_eq!(key(PhysicalKey::Code(KeyCode::Backquote), &chars("ё")), Key::Char('`'));
}

#[test]
fn latin_layouts_keep_the_logical_letter() {
    assert_eq!(key(PhysicalKey::Code(KeyCode::KeyQ), &chars("a")), Key::Char('A'));
    assert_eq!(key(PhysicalKey::Code(KeyCode::KeyC), &chars("j")), Key::Char('J'));
    assert_eq!(key(PhysicalKey::Code(KeyCode::Digit1), &chars("&")), Key::Char('1'));
    assert_eq!(key(PhysicalKey::Code(KeyCode::Equal), &chars("=")), Key::Char('+'));
}

#[test]
fn named_and_numpad_keys() {
    assert_eq!(key(PhysicalKey::Code(KeyCode::ArrowUp), &WinitKey::Named(NamedKey::ArrowUp)), Key::Up);
    assert_eq!(key(PhysicalKey::Code(KeyCode::Numpad8), &WinitKey::Named(NamedKey::ArrowUp)), Key::Up);
    assert_eq!(key(PhysicalKey::Code(KeyCode::F5), &WinitKey::Named(NamedKey::F5)), Key::F(5));
    assert_eq!(key(PhysicalKey::Code(KeyCode::Space), &chars(" ")), Key::Space);
    assert_eq!(key(PhysicalKey::Code(KeyCode::Numpad1), &chars("1")), Key::Other);
    assert_eq!(key(PhysicalKey::Code(KeyCode::NumpadAdd), &chars("+")), Key::NumPlus);
    assert_eq!(key(PhysicalKey::Code(KeyCode::ContextMenu), &WinitKey::Named(NamedKey::ContextMenu)), Key::Apps);
}

#[test]
fn resize_edges_and_corners() {
    let size = (800.0, 600.0);
    assert_eq!(resize_edge(400.0, 300.0, size, 5.0), None);
    assert_eq!(resize_edge(400.0, 2.0, size, 5.0), Some(ResizeDirection::North));
    assert_eq!(resize_edge(10.0, 2.0, size, 5.0), Some(ResizeDirection::NorthWest));
    assert_eq!(resize_edge(798.0, 590.0, size, 5.0), Some(ResizeDirection::SouthEast));
    assert_eq!(resize_edge(0.0, 300.0, size, 5.0), Some(ResizeDirection::West));
    assert_eq!(resize_edge(400.0, 599.0, size, 5.0), Some(ResizeDirection::South));
}

#[test]
fn clicks_alternate_single_and_double() {
    let mut c = Clicks::default();
    let t = Instant::now();
    let ms = Duration::from_millis;
    assert_eq!(c.press(MouseButton::Left, t, 10.0, 10.0, 1.0), 1);
    assert_eq!(c.press(MouseButton::Left, t + ms(100), 11.0, 10.0, 1.0), 2);
    assert_eq!(c.press(MouseButton::Left, t + ms(200), 11.0, 10.0, 1.0), 1);
    assert_eq!(c.press(MouseButton::Left, t + ms(300 + DOUBLE_CLICK.as_millis() as u64), 11.0, 10.0, 1.0), 1);
    assert_eq!(c.press(MouseButton::Right, t + ms(2000), 11.0, 10.0, 1.0), 1);
    assert_eq!(c.press(MouseButton::Left, t + ms(2050), 11.0, 10.0, 1.0), 1);
    assert_eq!(c.press(MouseButton::Left, t + ms(2100), 30.0, 10.0, 1.0), 1);
}

#[test]
fn wheel_notches_follow_windows_signs() {
    assert_eq!(wheel(MouseScrollDelta::LineDelta(0.0, 1.0), 1.0), (-0.0, 1.0));
    assert_eq!(wheel(MouseScrollDelta::LineDelta(1.0, 0.0), 1.0), (-1.0, 0.0));
}
