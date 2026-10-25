use super::*;

fn readable_on(t: &Theme, bg: Color) {
    assert!(contrast(t.text, bg) >= MIN_TEXT_CONTRAST, "text on {:08X}", bg.0);
    assert!(contrast(t.text_dim, bg) >= MIN_TEXT_CONTRAST, "dim text on {:08X}", bg.0);
    assert!(contrast(t.error, bg) >= MIN_TEXT_CONTRAST, "error on {:08X}", bg.0);
    assert!(contrast(t.accent, bg) >= MIN_ACCENT_CONTRAST, "accent on {:08X}", bg.0);
}

#[test]
fn the_default_palettes_are_the_fixed_light_and_dark_themes() {
    assert_eq!(Palette::new(false, Tint::Default, [Tint::Default; 5]).base, LIGHT);
    let dark = Palette::new(true, Tint::Default, [Tint::Default; 5]);
    assert!(dark.dark);
    assert_eq!(dark.base, DARK);
    assert_eq!(*dark.area(Area::Sidebar), DARK);
    assert!(Mode::System.dark(true) && !Mode::Light.dark(true) && Mode::Dark.dark(false));
}

#[test]
fn text_and_accents_stay_readable_on_every_preset_and_custom_color() {
    for dark in [false, true] {
        for (i, preset) in PRESETS.iter().enumerate() {
            let p = Palette::new(dark, Tint::Preset(i), [Tint::Default; 5]);
            assert_eq!(p.dark, dark, "{} keeps the theme", preset.id);
            readable_on(&p.base, p.base.bg);
            readable_on(&p.base, p.base.chrome);
        }
    }
    for r in (0..=255u32).step_by(51) {
        for g in (0..=255u32).step_by(17) {
            for b in (0..=255u32).step_by(51) {
                let rgb = r << 16 | g << 8 | b;
                let p = Palette::new(false, Tint::Custom(rgb), [Tint::Custom(rgb); 5]);
                assert_eq!(p.base.bg, Color::rgb(rgb));
                readable_on(&p.base, p.base.bg);
                for a in Area::ALL {
                    readable_on(p.area(a), Color::rgb(rgb));
                }
                assert_eq!(p.area(Area::Address).input_bg, Color::rgb(rgb));
                assert_eq!(p.area(Area::Sidebar).chrome, Color::rgb(rgb));
            }
        }
    }
}

#[test]
fn areas_follow_the_background_unless_they_have_their_own_color() {
    let p = Palette::new(false, Tint::Preset(5), [Tint::Default, Tint::Custom(0x202020), Tint::Default, Tint::Default, Tint::Preset(0)]);
    assert_eq!(*p.area(Area::Files), p.base);
    assert_eq!(p.area(Area::Toolbar).bg, Color::rgb(0x202020));
    assert!(contrast(p.area(Area::Toolbar).text, Color::rgb(0x202020)) > 10.0, "light text on a dark toolbar");
    assert_eq!(p.area(Area::Pane).bg, Color::rgb(0xFBF1D3));
    let night = Palette::new(true, Tint::Default, [Tint::Default, Tint::Default, Tint::Default, Tint::Default, Tint::Preset(0)]);
    assert_eq!(night.area(Area::Pane).bg, Color::rgb(0x2E2812), "presets have a dark shade for the dark theme");
}

#[test]
fn tints_parse_and_print_their_settings_value() {
    for s in ["", "gold", "Storm", "#1A2b3C", "#000000"] {
        let t = Tint::parse(s).unwrap();
        assert_eq!(Tint::parse(&t.key()), Some(t));
    }
    assert_eq!(Tint::parse("#1a2b3c"), Some(Tint::Custom(0x1A2B3C)));
    assert_eq!(Tint::parse("#1a2b3c").unwrap().key(), "#1A2B3C");
    assert_eq!(Tint::parse(" storm "), Some(Tint::Preset(10)));
    for bad in ["#12345", "#1234567", "#GGGGGG", "purple", "123456", "#+12345"] {
        assert_eq!(Tint::parse(bad), None, "{bad}");
    }
}
