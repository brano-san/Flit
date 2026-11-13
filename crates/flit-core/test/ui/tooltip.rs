use super::*;
use crate::input::Event;
use crate::ui::harness;

fn title(id: &str) -> String {
    ACTIONS[index(id).unwrap()].title.t().to_owned()
}

fn hover(a: &mut App, r: Rect, now: Instant) {
    a.on_event(&Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }, now);
}

#[test]
fn tooltips_tell_the_purpose_the_command_and_its_keys() {
    let a = harness::app(&["a.txt"]);
    let t = a.tip_text(Hit::NewTab).unwrap();
    assert_eq!(t.line, "Open a new tab in this panel.");
    assert_eq!(t.command, Some(title("tab.new")));
    assert_eq!(t.keys, ["Ctrl+T"]);
    assert_eq!(a.tip_text(Hit::Nav(0)).unwrap().keys, ["MouseX1", "Alt+Left"], "every shortcut gets a key cap, mouse buttons first");
    let view = a.tip_text(Hit::ViewMode).unwrap();
    assert_eq!((view.line.as_str(), view.command, view.keys), ("View settings of this folder are not saved.", Some(title("folder.cycle_view")), vec!["Blank".to_owned()]), "a command without a shortcut shows Blank");
    let min = a.tip_text(Hit::Caption(0)).unwrap();
    assert_eq!((min.line.as_str(), min.command, min.keys), ("Minimize the window.", Some(title("window.minimize")), vec!["Alt+M".to_owned()]));
    assert!(a.tip_text(Hit::Row(0)).is_some_and(|t| t.command.is_none()), "a row tip is plain text, not a command");
}

#[test]
fn toggle_buttons_describe_their_current_state() {
    let mut a = harness::app(&["a.txt"]);
    let shown = a.nav.sidebar.state.sidebar;
    let before = a.tip_text(Hit::QuickToggle).unwrap().line;
    a.toggle_sidebar();
    let after = a.tip_text(Hit::QuickToggle).unwrap().line;
    assert_ne!(before, after);
    assert_eq!(if shown { after } else { before }, "Quick Access is hidden.");
}

#[test]
fn a_tooltip_waits_for_the_delay_and_respects_the_setting() {
    let mut a = harness::app(&["a.txt"]);
    let t0 = Instant::now();
    let r = a.layout.new_tab;
    hover(&mut a, r, t0);
    assert_eq!(a.tooltip(), None, "nothing shows at once");
    a.on_event(&Event::Timer, t0 + DELAY);
    assert_eq!(a.tooltip().map(|t| t.line), Some("Open a new tab in this panel.".to_owned()));
    a.on_event(&Event::MouseDown { x: r.x + 1.0, y: r.y + 1.0, button: crate::input::MouseButton::Left, clicks: 1 }, t0 + DELAY);
    assert_eq!(a.tooltip(), None, "a press hides it");
    let mut a = harness::app(&["a.txt"]);
    let i = crate::ui::settings::index("appearance.tooltips").unwrap();
    assert!(crate::ui::SETTINGS[i].set(&mut a, crate::ui::Value::Bool(false)));
    hover(&mut a, r, t0);
    a.on_event(&Event::Timer, t0 + DELAY);
    assert_eq!(a.tooltip(), None, "Show Tooltips off suppresses them");
    assert!(a.tip.is_none());
}

#[test]
fn a_tooltip_fades_out_when_it_hides() {
    let mut a = harness::app(&["a.txt"]);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    let t0 = Instant::now();
    let r = a.layout.new_tab;
    hover(&mut a, r, t0);
    a.on_event(&Event::Timer, t0 + DELAY);
    a.frame(&mut g, t0 + DELAY + Duration::from_millis(300));
    let list = a.layout.list;
    let gone = t0 + DELAY + Duration::from_millis(310);
    a.on_event(&Event::MouseMove { x: list.x + list.w / 2.0, y: list.bottom() - 4.0 }, gone);
    assert_eq!(a.tooltip(), None);
    assert!(a.frame(&mut g, gone), "the tip fades out");
    assert_eq!(a.ghosts.fading_count(), 1);
    assert!(!a.frame(&mut g, gone + Duration::from_millis(200)));
    a.animations = false;
    hover(&mut a, r, gone);
    a.on_event(&Event::Timer, gone + DELAY);
    a.frame(&mut g, gone + DELAY);
    a.on_event(&Event::MouseMove { x: list.x + list.w / 2.0, y: list.bottom() - 4.0 }, gone + DELAY);
    assert!(!a.frame(&mut g, gone + DELAY + Duration::from_millis(10)), "without Animations it just goes");
    assert!(a.ghosts.fading_count() == 0);
}

fn long_name() -> String {
    format!("a-{}.txt", "wide-".repeat(60))
}

fn row_point(a: &App, row: usize) -> (f32, f32) {
    let r = a.row_tip_anchor(row).unwrap();
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn over_row(a: &mut App, row: usize, now: Instant) {
    let (x, y) = row_point(a, row);
    a.on_event(&Event::MouseMove { x, y }, now);
}

#[test]
fn a_cut_off_name_shows_in_full_after_the_delay() {
    let long = long_name();
    let mut a = harness::app(&[long.as_str(), "b-short.txt"]);
    assert_eq!(a.details.elided, [0], "only the wide name is cut off");
    let t0 = Instant::now();
    over_row(&mut a, 0, t0);
    assert_eq!(a.tooltip(), None, "nothing shows at once");
    a.on_event(&Event::Timer, t0 + DELAY);
    let tip = a.tooltip().unwrap();
    assert_eq!((tip.line.as_str(), tip.command, tip.keys, tip.details), (long.as_str(), None, Vec::new(), Vec::new()));
}

#[test]
fn a_name_that_fits_gets_type_size_and_date_and_a_folder_only_its_type() {
    let mut a = harness::app(&["notes.txt"]);
    let t0 = Instant::now();
    over_row(&mut a, 0, t0);
    a.on_event(&Event::Timer, t0 + DELAY);
    let tip = a.tooltip().unwrap();
    assert_eq!(tip.line, "Type: TXT File");
    assert_eq!(tip.details, ["Size: 2.0 KB"], "an unknown date is left out");
}

#[test]
fn row_tips_wait_again_for_every_row_and_stay_away_from_presses_drags_and_renames() {
    let mut a = harness::app(&["a.txt", "b.txt"]);
    let t0 = Instant::now();
    over_row(&mut a, 0, t0);
    a.on_event(&Event::Timer, t0 + DELAY);
    assert!(a.tooltip().is_some());
    let t1 = t0 + DELAY;
    over_row(&mut a, 1, t1);
    assert_eq!(a.tooltip(), None, "moving to the next row restarts the delay");
    a.on_event(&Event::Timer, t1 + DELAY);
    assert!(a.tooltip().is_some());
    let (x, y) = row_point(&a, 1);
    a.on_event(&Event::MouseDown { x, y, button: crate::input::MouseButton::Left, clicks: 1 }, t1 + DELAY);
    assert_eq!(a.tooltip(), None, "a press hides it");
    a.on_event(&Event::Wheel { dx: 0.0, dy: -1.0 }, t1 + DELAY);
    assert_eq!(a.tooltip(), None, "scrolling hides it");

    let mut a = harness::app(&["a.txt", "b.txt"]);
    a.on_event(&Event::Key { key: crate::input::Key::Down, mods: crate::input::Mods::default(), pressed: true }, t0);
    a.run_action("file.rename", t0);
    assert!(a.is_renaming());
    over_row(&mut a, 1, t0);
    a.on_event(&Event::Timer, t0 + DELAY);
    assert_eq!(a.tooltip(), None, "no tip while a name is being edited");
    assert_eq!(a.tip_text(Hit::Row(1)), None);
}

#[test]
fn row_tips_cover_every_layout() {
    use crate::ui::LayoutMode::*;
    let long = long_name();
    for mode in [Details, List, Cards, Columns, Grid, Tree] {
        let mut a = harness::app(&[long.as_str(), "b-short.txt"]);
        a.set_layout(mode);
        harness::frame(&mut a);
        let t0 = Instant::now();
        over_row(&mut a, 1, t0);
        a.on_event(&Event::Timer, t0 + DELAY);
        let tip = a.tooltip().unwrap_or_else(|| panic!("{mode:?}: no tip"));
        assert!(tip.line.starts_with("Type: ") || tip.line == "b-short.txt", "{mode:?}: {tip:?}");
        over_row(&mut a, 0, t0 + DELAY);
        a.on_event(&Event::Timer, t0 + 2 * DELAY);
        let tip = a.tooltip().unwrap_or_else(|| panic!("{mode:?}: no tip for the wide name"));
        if a.details.elided.contains(&0) {
            assert_eq!(tip.line, long, "{mode:?}");
        }
    }
}
