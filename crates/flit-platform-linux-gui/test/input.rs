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
    assert_eq!(key(PhysicalKey::Code(KeyCode::Equal), &chars("=")), Key::Char('='));
}

#[test]
fn named_and_numpad_keys() {
    assert_eq!(key(PhysicalKey::Code(KeyCode::ArrowUp), &WinitKey::Named(NamedKey::ArrowUp)), Key::Up);
    assert_eq!(key(PhysicalKey::Code(KeyCode::Numpad8), &WinitKey::Named(NamedKey::ArrowUp)), Key::Up);
    assert_eq!(key(PhysicalKey::Code(KeyCode::F5), &WinitKey::Named(NamedKey::F5)), Key::F(5));
    assert_eq!(key(PhysicalKey::Code(KeyCode::Space), &chars(" ")), Key::Space);
    assert_eq!(key(PhysicalKey::Code(KeyCode::Numpad1), &chars("1")), Key::Char('1'));
    assert_eq!(key(PhysicalKey::Code(KeyCode::NumpadAdd), &chars("+")), Key::NumPlus);
    assert_eq!(key(PhysicalKey::Code(KeyCode::NumpadSubtract), &chars("-")), Key::NumMinus);
    assert_eq!(key(PhysicalKey::Code(KeyCode::NumpadMultiply), &chars("*")), Key::NumStar);
    assert_eq!(key(PhysicalKey::Code(KeyCode::Minus), &chars("-")), Key::Char('-'));
    assert_eq!(key(PhysicalKey::Code(KeyCode::ContextMenu), &WinitKey::Named(NamedKey::ContextMenu)), Key::Apps);
}

#[test]
fn resize_edges_and_corners() {
    let size = (800.0, 600.0);
    assert_eq!(resize_edge(400.0, 300.0, size, 5.0, crate::tiling::Sides::ALL), None);
    assert_eq!(resize_edge(400.0, 2.0, size, 5.0, crate::tiling::Sides::ALL), Some(ResizeDirection::North));
    assert_eq!(resize_edge(10.0, 2.0, size, 5.0, crate::tiling::Sides::ALL), Some(ResizeDirection::NorthWest));
    assert_eq!(resize_edge(798.0, 590.0, size, 5.0, crate::tiling::Sides::ALL), Some(ResizeDirection::SouthEast));
    assert_eq!(resize_edge(0.0, 300.0, size, 5.0, crate::tiling::Sides::ALL), Some(ResizeDirection::West));
    assert_eq!(resize_edge(400.0, 599.0, size, 5.0, crate::tiling::Sides::ALL), Some(ResizeDirection::South));
}

#[test]
fn a_window_tiled_to_the_left_half_resizes_only_from_its_free_right_side() {
    use crate::tiling::Sides;
    let size = (800.0, 600.0);
    let left_half = Sides { top: false, bottom: false, left: false, right: true };
    assert_eq!(resize_edge(0.0, 300.0, size, 5.0, left_half), None);
    assert_eq!(resize_edge(400.0, 2.0, size, 5.0, left_half), None);
    assert_eq!(resize_edge(400.0, 599.0, size, 5.0, left_half), None);
    assert_eq!(resize_edge(1.0, 1.0, size, 5.0, left_half), None);
    assert_eq!(resize_edge(799.0, 300.0, size, 5.0, left_half), Some(ResizeDirection::East));
    assert_eq!(resize_edge(799.0, 2.0, size, 5.0, left_half), Some(ResizeDirection::East), "a locked top turns the corner into the free side");
    let top_locked = Sides { top: false, ..Sides::ALL };
    assert_eq!(resize_edge(2.0, 10.0, size, 5.0, top_locked), Some(ResizeDirection::West));
    assert_eq!(resize_edge(2.0, 595.0, size, 5.0, top_locked), Some(ResizeDirection::SouthWest));
    for (x, y) in [(0.0, 0.0), (799.0, 0.0), (0.0, 599.0), (799.0, 599.0), (400.0, 0.0), (0.0, 300.0)] {
        assert_eq!(resize_edge(x, y, size, 5.0, Sides::NONE), None);
    }
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

#[test]
fn a_ctrl_release_reaches_the_app_through_the_compositor_modifiers() {
    let ctrl = ModifiersState::CONTROL;
    let event = |mods: Mods| Some(Event::Key { key: Key::Other, mods, pressed: false });
    assert_eq!(modifiers_changed(ctrl, ModifiersState::empty()), event(Mods::default()));
    assert_eq!(modifiers_changed(ModifiersState::empty(), ctrl), event(Mods { ctrl: true, ..Mods::default() }));
    assert_eq!(modifiers_changed(ctrl, ctrl), None);
    assert_eq!(modifiers_changed(ModifiersState::META, ModifiersState::empty()), None, "keys Flit ignores change nothing");
}

#[test]
fn desktop_mouse_settings_come_from_kde_then_gtk() {
    let kde = "[General]\nfont=x\n\n[KDE]\nDoubleClickInterval=300\nWheelScrollLines=7\n";
    let gtk = vec!["[Settings]\ngtk-double-click-time = 250\n".to_owned()];
    assert_eq!(mouse_settings(Some(kde), &gtk), (Some(Duration::from_millis(300)), Some(7)));
    assert_eq!(mouse_settings(None, &gtk), (Some(Duration::from_millis(250)), None));
    assert_eq!(mouse_settings(Some("[KDE]\nWheelScrollLines=0\nDoubleClickInterval=9\n"), &[]), (None, None), "nonsense values keep the defaults");
    assert_eq!(mouse_settings(None, &["gtk-double-click-time=abc".to_owned()]), (None, None));
}

#[test]
fn a_slower_double_click_setting_widens_the_window() {
    let (t, ms) = (Instant::now(), Duration::from_millis);
    let mut c = Clicks::new(ms(900));
    assert_eq!(c.press(MouseButton::Left, t, 10.0, 10.0, 1.0), 1);
    assert_eq!(c.press(MouseButton::Left, t + ms(800), 10.0, 10.0, 1.0), 2);
    let mut c = Clicks::new(ms(200));
    assert_eq!(c.press(MouseButton::Left, t, 10.0, 10.0, 1.0), 1);
    assert_eq!(c.press(MouseButton::Left, t + ms(300), 10.0, 10.0, 1.0), 1);
}

#[test]
fn the_gnome_double_click_time_overrides_the_files_and_reaches_open_windows() {
    let (t, ms) = (Instant::now(), Duration::from_millis);
    assert_eq!(effective_click(700, Some(ms(250))), ms(700), "GNOME's own setting beats a stale settings.ini");
    assert_eq!(effective_click(0, Some(ms(250))), ms(250), "no answer keeps the file value");
    assert_eq!(effective_click(0, None), DOUBLE_CLICK);
    assert_eq!(effective_click(20, Some(ms(250))), ms(250), "a nonsense answer is ignored");
    assert_eq!(effective_click(9000, None), DOUBLE_CLICK);
    let mut c = Clicks::new(ms(200));
    c.set_interval(ms(900));
    assert_eq!(c.press(MouseButton::Left, t, 10.0, 10.0, 1.0), 1);
    assert_eq!(c.press(MouseButton::Left, t + ms(800), 10.0, 10.0, 1.0), 2, "a window already open uses the new time");
}
