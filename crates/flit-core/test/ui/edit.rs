use super::*;

const NONE: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };

#[test]
fn typing_and_deleting() {
    let mut e = LineEdit::default();
    e.set(r"C:\Users");
    e.insert(r"\Пётр");
    assert_eq!(e.text, r"C:\Users\Пётр");
    e.key(Key::Backspace, NONE);
    assert_eq!(e.text, r"C:\Users\Пёт");
    e.key(Key::Backspace, CTRL);
    assert_eq!(e.text, r"C:\Users\");
    e.key(Key::Home, NONE);
    e.key(Key::Delete, NONE);
    assert_eq!(e.text, r":\Users\");
}

#[test]
fn selection_and_words() {
    let mut e = LineEdit::default();
    e.set(r"C:\Program Files\App");
    e.key(Key::Left, CTRL);
    assert_eq!(e.caret, 17);
    e.key(Key::Left, CTRL);
    assert_eq!(e.caret, 11);
    e.key(Key::End, SHIFT);
    assert_eq!(&e.text[e.selection()], r"Files\App");
    assert_eq!(e.key(Key::Char('X'), CTRL), EditAction::Copy(r"Files\App".into()));
    assert_eq!(e.text, r"C:\Program ");
    e.select_all();
    e.insert("D:\n");
    assert_eq!(e.text, "D:", "control chars dropped");
    e.key(Key::Home, NONE);
    e.key(Key::Right, CTRL);
    assert_eq!(e.caret, 2);
}

#[test]
fn altgr_is_not_a_shortcut() {
    let altgr = Mods { ctrl: true, shift: false, alt: true };
    let mut e = LineEdit::default();
    e.set("zaba");
    e.key(Key::Left, NONE);
    for k in ['A', 'C', 'X', 'V'] {
        assert_eq!(e.key(Key::Char(k), altgr), EditAction::None);
    }
    assert!(e.selection().is_empty());
    e.insert("ą");
    assert_eq!(e.text, "zabąa");
}

#[test]
fn caret_snaps_to_char_boundary() {
    let mut e = LineEdit::default();
    e.set("Пётр");
    e.move_to(3, false);
    assert_eq!(e.caret, 2);
    e.move_to(1, true);
    assert_eq!(&e.text[e.selection()], "П");
}
