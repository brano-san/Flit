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

#[test]
fn insert_and_delete_chords_copy_paste_and_cut() {
    let mut e = LineEdit::default();
    e.set("one two");
    e.key(Key::Left, CTRL);
    e.key(Key::End, SHIFT);
    assert_eq!(e.key(Key::Insert, CTRL), EditAction::Copy("two".into()));
    assert_eq!(e.key(Key::Insert, SHIFT), EditAction::Paste);
    assert_eq!(e.text, "one two", "Ctrl+Insert and Shift+Insert leave the text alone");
    assert_eq!(e.key(Key::Delete, SHIFT), EditAction::Copy("two".into()));
    assert_eq!(e.text, "one ");
    assert_eq!(e.key(Key::Delete, SHIFT), EditAction::None, "nothing selected, nothing cut");
    assert_eq!(e.text, "one ");
    e.key(Key::Char('Z'), CTRL);
    assert_eq!(e.text, "one two");
    assert_eq!(e.key(Key::Insert, Mods { ctrl: true, shift: true, alt: false }), EditAction::None);
}

#[test]
fn a_word_is_picked_around_the_position() {
    let mut e = LineEdit::default();
    e.set(r"C:\Program Files\my-app.exe");
    e.select_word(5);
    assert_eq!(&e.text[e.selection()], "Program");
    e.select_word(16);
    assert_eq!(&e.text[e.selection()], "Files", "the end of a word still picks that word");
    e.select_word(11);
    assert_eq!(&e.text[e.selection()], "Files", "the start of a word picks the word after it");
    e.select_word(e.text.len());
    assert_eq!(&e.text[e.selection()], "exe");
    e.set("a   b");
    e.select_word(2);
    assert_eq!(e.selection(), 1..4, "between words the run of spaces is picked");
    e.set("");
    e.select_word(0);
    assert!(e.selection().is_empty());
}

#[test]
fn clicks_map_to_caret_word_and_everything() {
    assert_eq!(Grab::of(1, false), Some(Grab::Caret));
    assert_eq!(Grab::of(1, true), Some(Grab::Extend));
    assert_eq!(Grab::of(2, true), Some(Grab::Word));
    assert_eq!(Grab::of(3, false), None);
    let mut e = LineEdit::default();
    e.set("alpha beta");
    e.grab(2, Grab::Caret);
    e.grab(8, Grab::Extend);
    assert_eq!(&e.text[e.selection()], "pha be");
    e.grab(8, Grab::Word);
    assert_eq!(&e.text[e.selection()], "beta");
    assert!(!e.can_undo());
}

#[test]
fn a_secret_field_never_hands_out_its_text() {
    use crate::ui::field::{Field, FieldKey};
    let mut f = Field::default();
    f.secret = true;
    f.insert("hunter2");
    f.edit.select_all();
    for (k, m) in [(Key::Char('C'), CTRL), (Key::Char('X'), CTRL), (Key::Insert, CTRL), (Key::Delete, SHIFT)] {
        assert!(matches!(f.key(k, m), FieldKey::Edited { changed: false, action: EditAction::None }));
    }
    assert_eq!(f.text(), "hunter2");
    assert!(matches!(f.key(Key::Insert, SHIFT), FieldKey::Edited { action: EditAction::Paste, .. }));
}
