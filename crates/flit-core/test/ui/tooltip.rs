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
    let home = a.tip_text(Hit::HomeButton).unwrap();
    assert_eq!((home.command, home.keys), (Some(title("nav.home_page")), vec!["Blank".to_owned()]), "a command without a shortcut shows Blank");
    let min = a.tip_text(Hit::Caption(0)).unwrap();
    assert_eq!((min.line.as_str(), min.command, min.keys), ("Minimize the window.", Some(title("window.minimize")), vec!["Alt+M".to_owned()]));
    assert_eq!(a.tip_text(Hit::Row(0)), None, "rows are not buttons");
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
