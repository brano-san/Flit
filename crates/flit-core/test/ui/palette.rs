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
    assert_eq!(titles("sort size")[0], "Sort By Size");
    assert!(titles("refresh")[..2].contains(&"Refresh"));
    assert!(titles("bookmark")[..2].contains(&"Toggle Folder Bookmark"));
    assert_eq!(titles("filter")[0], "Filter");
    assert!(titles("qqq").is_empty());
}

#[test]
fn commands_are_found_by_words_of_their_descriptions() {
    assert!(ACTIONS.iter().all(|a| !super::super::describe::description(a.id).is_empty()), "every command has a description");
    assert_eq!(titles("split right")[0], "Split Right", "a title match ranks first");
    assert!(titles("current panel").contains(&"Split Right"));
    assert!(titles("recycle bin").contains(&"Trash"));
    assert!(titles("unzip").contains(&"Extract here"));
    assert!(titles("checksum").contains(&"Hashes…"));
    assert!(titles("trash qqq").is_empty(), "every word must match");
}

fn commands(a: &App) -> &Commands {
    a.nav.palette.as_ref().unwrap().commands.as_ref().unwrap()
}

fn headers(a: &App) -> Vec<Group> {
    commands(a).lines.iter().filter_map(|l| if let Line::Header(g) = l { Some(*g) } else { None }).collect()
}

fn item_title(a: &App, row: usize) -> String {
    let p = a.nav.palette.as_ref().unwrap();
    match &commands(a).lines[row] {
        Line::Item(k, ..) => p.titles[*k].clone(),
        Line::Header(g) => panic!("row {row} is the header of {g:?}"),
    }
}

#[test]
fn ctrl_shift_p_lists_commands_under_collapsible_group_headers() {
    let mut a = crate::ui::harness::app(&["a.txt"]);
    let now = Instant::now();
    a.on_event(&Event::Key { key: Key::Char('P'), mods: Mods { ctrl: true, shift: true, alt: false }, pressed: true }, now);
    assert_eq!(a.nav.palette.as_ref().map(|p| p.pick.clone()), Some(Pick::Commands));
    assert_eq!(headers(&a), Group::ALL, "every group has its header, in the order of the registry");
    assert_eq!(item_title(&a, 1), "Toggle Subfolders");
    let all = commands(&a).lines.len();
    a.run_palette_item(0, now);
    assert!(commands(&a).collapsed.contains(&Group::Quick));
    assert_eq!(commands(&a).lines[1], Line::Header(Group::Folder), "a collapsed group shows only its header");
    a.run_palette_item(0, now);
    assert_eq!(commands(&a).lines.len(), all);
}

#[test]
fn the_description_toggle_is_remembered() {
    let mut a = crate::ui::harness::app(&["a.txt"]);
    assert!(a.prefs.command_descriptions, "descriptions are shown at first");
    a.open_palette();
    a.toggle_descriptions();
    assert!(!commands(&a).descriptions && !a.prefs.command_descriptions);
    a.close_palette();
    a.open_palette();
    assert!(!commands(&a).descriptions);
    a.toggle_descriptions();
    assert!(commands(&a).descriptions && a.prefs.command_descriptions);
}

#[test]
fn a_query_ranks_commands_fuzzily_and_marks_titles() {
    let mut a = crate::ui::harness::app(&["a.txt"]);
    a.open_palette();
    a.palette_insert("tghid");
    assert!(headers(&a).is_empty(), "a query shows one flat list");
    assert_eq!(item_title(&a, 0), "Toggle Hidden Files");
    let Line::Item(_, marks, _) = &commands(&a).lines[0] else { unreachable!() };
    assert!(!marks.is_empty());
    a.palette_insert("qqqq");
    assert!(commands(&a).lines.is_empty());
}

#[test]
fn unavailable_commands_are_listed_gray_and_never_run() {
    let mut a = crate::ui::harness::app(&["a.txt"]);
    a.open_palette();
    a.palette_insert("toggle subfolders");
    assert_eq!(item_title(&a, 0), "Toggle Subfolders");
    let Line::Item(k, ..) = commands(&a).lines[0] else { unreachable!() };
    assert!(!commands(&a).live[k]);
    a.drain_effects().for_each(drop);
    a.run_palette_item(0, Instant::now());
    assert!(a.nav.palette.is_some(), "picking a gray command keeps the palette open");
    assert!(a.drain_effects().next().is_none() && !a.search_active());
}
