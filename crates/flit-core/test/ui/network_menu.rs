use super::*;
use crate::ui::{MenuItem, SavedServer, ShellMenu};

fn context(app: &mut App, row: Option<&str>) -> ShellMenu {
    app.layout();
    let (x, y) = match row {
        Some(label) => coords(app, label),
        None => (app.layout.list.x + 80.0, app.layout.list.bottom() - 12.0),
    };
    down(app, (x, y), MouseButton::Right, 1);
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, Instant::now());
    app.drain_effects().find_map(|e| if let Effect::ShellMenu(menu) = e { Some(menu) } else { None }).expect("context menu")
}

fn menu_command(menu: &ShellMenu, title: S) -> Option<usize> {
    let k = menu.menu.iter().position(|i| matches!(i, MenuItem::Command { label, disabled: false, .. } if label == title.t()))?;
    Some(crate::ui::own_menu::commands(&menu.menu[..k]))
}

fn palette_row(app: &mut App) -> Option<usize> {
    app.open_palette();
    let p = app.nav.palette.as_ref().unwrap();
    (0..p.matches.len()).find(|&r| p.row_action(r).is_some_and(|i| crate::ui::actions::ACTIONS[i].id == "drive.add_server"))
}

#[test]
fn add_server_from_the_background_or_command_palette_opens_with_saving_selected() {
    for from_menu in [true, false] {
        let mut a = displayed_network();
        if from_menu {
            let menu = context(&mut a, None);
            let k = menu_command(&menu, S::ActionsDriveAddServer).expect("Add server in Network");
            a.on_event(&Event::MenuCommand(k), Instant::now());
        } else {
            let row = palette_row(&mut a).expect("Add server in command palette");
            a.run_palette_item(row, Instant::now());
        }
        assert!(a.dialogs.form.as_ref().unwrap().controls.iter().any(|c| matches!(c, Control::Check { label, on: true } if label == S::SidebarDriveSaveServer.t())));
    }
}

#[test]
fn adding_a_server_is_absent_in_windows_network_and_in_a_linux_folder() {
    let mut windows = app();
    windows.drain_effects().for_each(drop);
    in_network(&mut windows);
    publish(&mut windows, true, &[], None);
    let mut folder = linux();
    folder.navigate(Location::parse("/tmp").unwrap(), Instant::now());
    publish(&mut folder, true, &[], None);
    for mut a in [windows, folder] {
        assert!(menu_command(&context(&mut a, None), S::ActionsDriveAddServer).is_none());
        assert!(palette_row(&mut a).is_none());
        assert!(!a.run_action("drive.add_server", Instant::now()));
    }
}

#[test]
fn the_saved_server_menu_renames_and_removes_only_the_bookmarked_address() {
    let mut a = linux();
    let uri = "sftp://ann@192.168.10.4";
    a.nav.sidebar.prefs.saved_servers.push(SavedServer { address: uri.into(), name: Some("Old".into()) });
    in_network(&mut a);
    publish(&mut a, true, &SERVERS, None);
    assert!(menu_command(&context(&mut a, Some("BRANO")), S::SidebarDriveRemoveServer).is_none());
    let menu = context(&mut a, Some("Old"));
    let rename = menu_command(&menu, S::MenuRename).expect("Rename the saved server");
    a.on_event(&Event::MenuCommand(rename), Instant::now());
    let form = a.dialogs.form.as_mut().expect("rename form");
    form.set_text(0, "New");
    a.form_submit(0);
    assert_eq!(a.nav_prefs().saved_servers, [SavedServer { address: uri.into(), name: Some("New".into()) }]);
    let menu = context(&mut a, Some("New"));
    let remove = menu_command(&menu, S::SidebarDriveRemoveServer).expect("Remove from Network");
    a.on_event(&Event::MenuCommand(remove), Instant::now());
    assert!(a.nav_prefs().saved_servers.is_empty());
    assert!(!rows(&a).iter().any(|label| label == "New"));
    assert!(launches(&mut a).is_empty());
}
