use super::*;
use crate::input::Event;
use crate::text::tests::Mono;
use std::time::Duration;

fn draw(a: &mut App, now: Instant) {
    a.frame(&mut Gfx::new(Mono, 1024), now);
}

fn long_pick(now: Instant) -> App {
    let mut a = crate::ui::harness::app(&[]);
    let rows = (0..200).map(|i| (format!("row {i}"), String::new())).collect();
    a.open_pick(Palette::new(Pick::NewItem, rows, ""));
    draw(&mut a, now);
    a
}

fn pos(a: &App) -> f32 {
    a.nav.palette.as_ref().unwrap().scroll.pos
}

fn wheel_frame(a: &mut App, dy: f32, now: Instant) -> f32 {
    a.on_event(&Event::Wheel { dx: 0.0, dy }, now);
    assert!(a.dirty, "a wheel over the palette must request a frame");
    draw(a, now + Duration::from_millis(16));
    pos(a)
}

#[test]
fn palette_wheel_up_and_down_move_on_the_same_frame() {
    let t0 = Instant::now();
    let mut a = long_pick(t0);
    let down = wheel_frame(&mut a, -1.0, t0);
    assert!(down > 0.0, "wheel down moves on its first frame");
    for k in 1..60 {
        draw(&mut a, t0 + Duration::from_millis(16 * k));
    }
    let settled = pos(&a);
    assert!(settled > down, "the palette glides toward the target over frames");
    let up = wheel_frame(&mut a, 1.0, t0 + Duration::from_millis(16 * 59));
    assert!(up < settled, "wheel up moves on its first frame: {settled} -> {up}");
    assert!((settled - up - down).abs() < 1.0, "one notch up and one notch down cover the same first-frame distance");
}

#[test]
fn palette_lays_out_only_visible_rows() {
    let t0 = Instant::now();
    let mut a = long_pick(t0);
    wheel_frame(&mut a, -30.0, t0);
    let p = a.nav.palette.as_ref().unwrap();
    assert!(p.rows.len() <= p.page + 1);
}

fn titles(q: &str) -> Vec<&'static str> {
    matches(q).into_iter().map(|i| ACTIONS[i].title.t()).collect()
}

#[test]
fn ranking_is_fuzzy_and_marks_the_matched_letters() {
    let rows = |q: &str, items: &[(&'static str, &'static str, &'static str)]| rank(q, items.iter().copied());
    let three = [("New tab", "", ""), ("Print", "", ""), ("Clear selection", "", "")];
    assert_eq!(rows("ntb", &three), vec![(0, vec![0..1, 4..5, 6..7])], "letters in order, gaps allowed");
    assert_eq!(rows("", &three).len(), 3);
    assert!(rows("xyz", &three).is_empty());
    let sel = rows("sel", &[("Clear selection", "", ""), ("Select all", "", "")]);
    assert_eq!(sel[0], (1, std::iter::once(0..3).collect()), "a match at a word start ranks first");
    assert_eq!(rows("dual", &[("Split Right", "", "dual pane")]), vec![(0, Vec::new())], "descriptions match without marks in the title");
    assert!(rows("dp", &[("Split Right", "", "dual pane")]).is_empty(), "scattered letters do not match descriptions");
    assert_eq!(rows("new", &[("Nouvel onglet", "New tab", "")]), vec![(0, Vec::new())], "the English title matches too");
}

#[test]
fn the_palette_sits_over_the_panels_at_half_the_window() {
    let mut a = crate::ui::harness::app(&["a.txt"]);
    a.open_palette();
    draw(&mut a, Instant::now());
    let (w, h) = a.size;
    let left = a.panels.left;
    let r = a.nav.palette.as_ref().unwrap().rect;
    assert!((r.w - w / 2.0).abs() <= 1.0, "half the window: {r:?}");
    assert!((r.x + r.w / 2.0 - (left + (w - left) / 2.0)).abs() <= 1.0, "centered over the panels: {r:?}");
    assert_eq!(r.y, a.s(TAB_H) + a.s(TOOL_H) + a.s(TOP_GAP));
    assert!(r.bottom() <= h);
}

#[test]
fn matches_rank_registered_titles() {
    assert_eq!(matches(""), (0..ACTIONS.len()).collect::<Vec<_>>());
    assert_eq!(titles("sort size")[0], "Sort by size");
    assert_eq!(titles("refresh")[0], "Refresh");
    assert_eq!(titles("pin")[0], "Pin or unpin the current folder");
    assert_eq!(titles("filter")[0], "Filter items in this folder");
    assert!(titles("qqq").is_empty());
}

#[test]
fn commands_are_found_by_words_of_their_descriptions() {
    assert!(ACTIONS.iter().all(|a| !super::super::describe::description(a.id).is_empty()), "every command has a description");
    assert_eq!(titles("split right")[0], "Split Right", "a title match ranks first");
    assert!(titles("dual pane").contains(&"Split Right"));
    assert!(titles("trash").contains(&"Move to Recycle Bin"));
    assert!(titles("unzip").contains(&"Extract here"));
    assert!(titles("checksum").contains(&"Hashes…"));
    assert!(titles("trash qqq").is_empty(), "every word must match");
}
