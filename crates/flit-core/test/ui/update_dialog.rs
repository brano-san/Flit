use super::*;
use crate::input::{Event, MouseButton};
use crate::text::tests::Mono;
use crate::ui::Persisted;
use crate::update::{Release, UpdateEvent};

fn app(w: u32, h: u32, scale: f32) -> App {
    let mut a = App::new(Persisted::default(), Vec::new(), 0, false, scale, (w, h), Instant::now());
    a.animations = false;
    let notes = format!("## Fixed\n{}", (0..120).map(|i| format!("- Изменение {i}: длинный текст со словами и проверкой полного переноса строки без потери содержимого.\n")).collect::<String>());
    a.on_update(UpdateEvent::Checked { manual: true, found: Ok(Some(Release { version: "99.0.0".into(), notes, url: String::new(), signature: String::new() })) });
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    a
}

#[test]
fn wrapping_preserves_unicode_long_words_and_every_word() {
    let mut g = Gfx::new(Mono, 1024);
    let font = FontStyle::new(FontFamily::Ui, 13.0, false);
    for text in ["слова с пробелами для переноса", "超长文件名称没有空格也不会丢失字符", "abcdefghijklmnopqrstuvwxyz0123456789", "e\u{301}clair and another word"] {
        let lines = wrapped(&mut g, text, font, 80.0);
        assert!(lines.iter().all(|s| g.measure(s, font) <= 80.0));
        assert_eq!(lines.concat().split_whitespace().collect::<String>(), text.split_whitespace().collect::<String>());
    }
}

#[test]
fn long_changelog_scrolls_without_moving_header_or_footer_at_any_scale() {
    for (w, h, scale) in [(1000, 800, 1.0), (600, 400, 1.0), (400, 260, 1.0), (800, 600, 2.0), (1200, 900, 1.5)] {
        let mut a = app(w, h, scale);
        let f = a.jobs.form.as_ref().unwrap();
        let bounds = f.rect;
        let buttons = f.button_rects.clone();
        let header = f.rows[..2].to_vec();
        assert!(bounds.y >= 0.0 && bounds.bottom() <= h as f32);
        assert!(f.document.max() > 0.0);
        assert!(f.document.lines.iter().any(|l| l.text.contains("119")));
        assert!(f.document.viewport.bottom() < buttons[0].y);
        assert!(buttons.iter().all(|r| r.y >= bounds.y && r.bottom() <= bounds.bottom() && r.x >= bounds.x && r.right() <= bounds.right()));
        a.on_event(&Event::Wheel { dx: 0.0, dy: -4.0 }, Instant::now());
        a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
        assert!(a.jobs.form.as_ref().unwrap().document.scroll.pos > 0.0);
        a.update_scroll_key(Key::End, Mods::default());
        let f = a.jobs.form.as_ref().unwrap();
        assert_eq!(f.document.scroll.pos, f.document.max());
        assert_eq!(f.rect, bounds);
        assert_eq!(f.button_rects, buttons);
        assert_eq!(&f.rows[..2], &header);
        a.update_scroll_key(Key::Home, Mods::default());
        assert_eq!(a.jobs.form.as_ref().unwrap().document.scroll.pos, 0.0);
    }
}

#[test]
fn scrollbar_drag_and_copy_feedback_preserve_dialog_state() {
    let mut a = app(1000, 800, 1.0);
    let f = a.jobs.form.as_ref().unwrap();
    let thumb = f.document.thumb();
    let end = f.document.track().bottom();
    let note = f.note.clone();
    let buttons = f.button_rects.clone();
    let value = f.rows[0];
    a.on_event(&Event::MouseDown { x: thumb.x + 2.0, y: thumb.y + 2.0, button: MouseButton::Left, clicks: 1 }, Instant::now());
    assert!(a.jobs.form.as_ref().unwrap().document.grab.is_some());
    a.on_event(&Event::MouseMove { x: thumb.x + 2.0, y: end }, Instant::now());
    let f = a.jobs.form.as_ref().unwrap();
    assert_eq!(f.document.scroll.pos, f.document.max());
    a.on_event(&Event::MouseUp { x: thumb.x, y: end, button: MouseButton::Left }, Instant::now());
    assert!(a.jobs.form.as_ref().unwrap().document.grab.is_none());
    a.form_down(value.x + 2.0, value.y + 2.0, 1);
    a.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let f = a.jobs.form.as_ref().unwrap();
    assert!(f.document.copied.is_some());
    assert_eq!(f.note, note);
    assert_eq!(f.button_rects, buttons);
    assert!(a.drain_effects().any(|e| matches!(e, super::super::Effect::SetClipboard(_))));
}
