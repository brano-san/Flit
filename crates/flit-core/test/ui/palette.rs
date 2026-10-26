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
    assert!(a.nav.palette.as_ref().unwrap().rows.len() <= MAX_ROWS + 1);
}

fn titles(q: &str) -> Vec<&'static str> {
    matches(q).into_iter().map(|i| ACTIONS[i].title.t()).collect()
}

#[test]
fn substrings_beat_fuzzy_matches_and_word_starts_win() {
    assert_eq!(score("", "Anything"), Some(0));
    assert!(score("tab", "New tab").unwrap() > score("nt", "New tab").unwrap());
    assert!(score("new", "New tab").unwrap() > score("ew", "New tab").unwrap());
    assert!(score("tab", "New tab").unwrap() > 2000);
    assert!(score("nt", "New tab").unwrap() < 2000);
    assert!(score("netab", "New tab").unwrap() > score("nt", "New tab").unwrap());
    assert_eq!(score("ntb", "New tab"), None, "fuzzy letters must start a word or follow the previous match");
    assert_eq!(score("sel", "Clear selection"), Some(3994));
    assert_eq!(score("so", "Clear selection"), None);
    assert!(score("so", "Show or hide hidden files").is_some());
    assert_eq!(score("xyz", "New tab"), None);
    assert_eq!(score("  NEW TAB ", "New tab"), score("new tab", "New tab"));
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
    let dual = titles("dual pane");
    assert_eq!(dual[0], "Toggle dual pane", "a title match ranks first");
    assert!(dual.contains(&"Split vertically: open this folder in a pane on the right"));
    assert!(titles("trash").contains(&"Move to Recycle Bin"));
    assert!(titles("unzip").contains(&"Extract here"));
    assert!(titles("checksum").contains(&"Hashes…"));
    assert!(titles("trash qqq").is_empty(), "every word must match");
}
