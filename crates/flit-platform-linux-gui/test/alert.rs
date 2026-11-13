use super::*;

#[test]
fn a_fatal_error_is_offered_to_the_desktop_dialog_tools_in_turn() {
    let message = "Flit cannot start: <no display> & more";
    let tools = dialogs(message);
    assert_eq!(tools.iter().map(|t| t[0].as_str()).collect::<Vec<_>>(), ["zenity", "kdialog", "notify-send"]);
    assert!(tools[0].contains(&format!("--text={message}")), "zenity takes the text whole and without markup");
    assert!(tools[0].iter().any(|a| a == "--no-markup"));
    for tool in &tools[1..] {
        assert_eq!(tool.last().map(String::as_str), Some(message), "the message is passed as one argument");
    }
}
