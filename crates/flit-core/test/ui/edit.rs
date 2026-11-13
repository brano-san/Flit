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

#[test]
fn case_changes_only_the_selection_and_keeps_its_direction() {
    let mut e = LineEdit::default();
    e.set("Straße.txt");
    assert!(!e.change_case(true), "nothing is selected");
    e.key(Key::Home, NONE);
    e.move_to(7, true);
    assert!(e.change_case(true));
    assert_eq!(e.text, "STRASSE.txt");
    assert_eq!((e.anchor, e.caret), (0, 7));
    e.set("ABC");
    e.move_to(1, true);
    assert!(e.change_case(false));
    assert_eq!(e.text, "Abc");
    assert_eq!((e.anchor, e.caret), (3, 1));
    assert!(!e.change_case(false), "already lowercase");
}

fn typed(e: &mut LineEdit, s: &str) {
    s.chars().for_each(|c| e.insert(&c.to_string()));
}

#[test]
fn typing_runs_undo_as_one_step_and_redo_restores_them() {
    let mut e = LineEdit::default();
    typed(&mut e, "hello");
    e.insert(" big world");
    typed(&mut e, "!");
    assert_eq!(e.key(Key::Char('Z'), CTRL), EditAction::Changed);
    assert_eq!(e.text, "hello big world");
    e.key(Key::Char('Z'), CTRL);
    assert_eq!(e.text, "hello");
    e.key(Key::Char('Z'), CTRL);
    assert_eq!(e.text, "");
    assert_eq!(e.key(Key::Char('Z'), CTRL), EditAction::None);
    e.key(Key::Char('Y'), CTRL);
    assert_eq!(e.text, "hello");
    e.key(Key::Char('Z'), Mods { ctrl: true, shift: true, alt: false });
    assert_eq!(e.text, "hello big world");
    typed(&mut e, "?");
    assert_eq!(e.key(Key::Char('Y'), CTRL), EditAction::None, "new edits drop the redo history");
}

#[test]
fn deleting_cutting_and_moving_break_the_runs() {
    let mut e = LineEdit::default();
    e.set("one two three");
    e.key(Key::Backspace, CTRL);
    e.key(Key::Backspace, NONE);
    e.key(Key::Backspace, NONE);
    assert_eq!(e.text, "one tw");
    e.key(Key::Char('Z'), CTRL);
    assert_eq!(e.text, "one two ", "plain backspaces group");
    e.key(Key::Char('Z'), CTRL);
    assert_eq!(e.text, "one two three");
    assert_eq!((e.caret, e.anchor), (13, 13));
    e.select_all();
    assert_eq!(e.key(Key::Char('X'), CTRL), EditAction::Copy("one two three".into()));
    assert_eq!(e.text, "");
    e.key(Key::Char('Z'), CTRL);
    assert_eq!((e.text.as_str(), e.selection()), ("one two three", 0..13));
    e.move_to(3, false);
    e.insert("X");
    e.key(Key::Left, NONE);
    e.insert("Y");
    e.key(Key::Char('Z'), CTRL);
    assert_eq!(e.text, "oneX two three");
    e.key(Key::Char('Z'), CTRL);
    assert_eq!(e.text, "one two three");
}

#[test]
fn empty_selection_copy_and_cut_do_nothing_and_history_is_capped() {
    let mut e = LineEdit::default();
    e.set("abc");
    assert_eq!(e.key(Key::Char('C'), CTRL), EditAction::None);
    assert_eq!(e.key(Key::Char('X'), CTRL), EditAction::None);
    assert_eq!(e.text, "abc");
    assert_eq!(e.key(Key::Char('Z'), CTRL), EditAction::None, "a no-op cut leaves no history");
    for _ in 0..500 {
        e.insert("word");
        e.key(Key::Left, NONE);
    }
    let mut steps = 0;
    while e.undo() {
        steps += 1;
    }
    assert!(steps <= 200 && steps > 100);
}
