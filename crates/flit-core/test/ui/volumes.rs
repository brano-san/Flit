use super::*;
use crate::input::Event;
use crate::location::Location;
use crate::ui::{Hit, MenuItem, Persisted, Places, ShellMenu};
use std::sync::Arc;
use std::time::Instant;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn disk(path: &str, label: &str, file_system: &str, unmounted: bool, removable: bool, remote: Option<&str>) -> Drive {
    Drive { path: p(path), label: label.into(), total: 0, free: 0, file_system: file_system.into(), removable, unmounted, kind: None, remote: remote.map(Into::into) }
}

fn linux_app(drives: Vec<Drive>) -> App {
    let mut app = App::new(Persisted { path: Location::home_page(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    app.set_caps(crate::caps::Caps { unix_paths: true });
    app.set_places(Arc::new(Places { drives, ..Default::default() }));
    app.drain_effects().for_each(drop);
    app
}

fn machine() -> App {
    linux_app(vec![
        disk("/", "File System", "ext4", false, false, None),
        disk("/home", "home", "ext4", false, false, None),
        disk("/media/ann/STICK", "STICK", "vfat", false, true, None),
        disk("/mnt/data", "data", "ext4", false, false, None),
        disk("/mnt/nas", "nas", "cifs", false, false, Some("//nas/share")),
        disk("/dev/sdc1", "Backup", "ext4", true, true, None),
        disk("/dev/sdd2", "Encrypted volume (sdd2)", "crypto_LUKS", true, true, None),
    ])
}

fn launches(app: &mut App) -> Vec<Launch> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect()
}

fn side(app: &App, path: &str) -> usize {
    app.nav.sidebar.items.iter().position(|it| it.loc.path() == &p(path)).unwrap_or_else(|| panic!("{path} not in the sidebar"))
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, .. } => vec![label.clone()],
            MenuItem::Submenu { items, .. } => labels(items),
            MenuItem::Header(_) | MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn menu_for(app: &mut App, path: &str) -> ShellMenu {
    let i = side(app, path);
    app.side_menu(10.0, 10.0, Hit::Side(i));
    app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).expect("menu shown")
}

fn drive_verbs(app: &mut App, path: &str) -> Vec<String> {
    let verbs = [S::MenuEject.t(), S::MenuUnmount.t(), S::MenuMount.t(), S::SidebarDriveUnlock.t(), S::SidebarDriveDisconnect.t()];
    labels(&menu_for(app, path).menu).into_iter().filter(|l| verbs.contains(&l.as_str())).collect()
}

#[test]
fn a_mounted_drive_can_be_unmounted_and_the_system_and_network_drives_cannot() {
    let mut app = machine();
    assert_eq!(drive_verbs(&mut app, "/media/ann/STICK"), [S::MenuEject.t(), S::MenuUnmount.t()]);
    assert_eq!(drive_verbs(&mut app, "/mnt/data"), [S::MenuUnmount.t()]);
    assert!(drive_verbs(&mut app, "/").is_empty());
    assert!(drive_verbs(&mut app, "/home").is_empty());
    assert!(!drive_verbs(&mut app, "/mnt/nas").contains(&S::MenuUnmount.t().to_owned()));
    assert_eq!(drive_verbs(&mut app, "/dev/sdc1"), [S::MenuMount.t()]);
    assert_eq!(drive_verbs(&mut app, "/dev/sdd2"), [S::SidebarDriveUnlock.t()]);
}

#[test]
fn unmount_from_the_menu_asks_the_platform_to_unmount_that_drive_only() {
    let mut app = machine();
    let m = menu_for(&mut app, "/media/ann/STICK");
    let k = labels(&m.menu).iter().position(|l| l == S::MenuUnmount.t()).unwrap();
    app.on_event(&Event::MenuCommand(k), Instant::now());
    assert_eq!(launches(&mut app), [Launch::Unmount(p("/media/ann/STICK"))]);
}

#[test]
fn windows_drives_get_no_unmount_or_mount_items() {
    let mut app = linux_app(vec![disk(r"C:\", "Local Disk (C:)", "NTFS", false, false, None), disk(r"E:\", "USB (E:)", "FAT32", false, true, None)]);
    assert!(drive_verbs(&mut app, r"C:\").is_empty());
    assert_eq!(drive_verbs(&mut app, r"E:\"), [S::MenuEject.t()]);
}

#[test]
fn opening_a_locked_volume_asks_for_its_passphrase_and_unlocks_it_with_that() {
    let mut app = machine();
    let i = side(&app, "/dev/sdd2");
    app.open_side_item(i, false, Instant::now());
    assert!(launches(&mut app).is_empty(), "nothing is mounted before the passphrase is given");
    let f = app.jobs.form.as_ref().expect("the passphrase is asked");
    assert!(matches!(&f.kind, Kind::Unlock { device } if *device == p("/dev/sdd2")));
    assert!(matches!(&f.controls[0], Control::Value { value, .. } if value == "Encrypted volume (sdd2)"));
    app.form_submit(0);
    assert!(app.jobs.form.is_some() && launches(&mut app).is_empty(), "an empty passphrase keeps the dialog open");
    app.jobs.form.as_mut().unwrap().set_text(1, "correct horse");
    app.form_submit(0);
    assert!(app.jobs.form.is_none());
    assert_eq!(launches(&mut app), [Launch::Unlock { device: p("/dev/sdd2"), passphrase: Secret("correct horse".into()) }]);
    assert!(!format!("{:?}", Launch::Unlock { device: p("/dev/sdd2"), passphrase: Secret("correct horse".into()) }).contains("horse"));
}

#[test]
fn cancelling_the_passphrase_unlocks_nothing_and_a_plain_volume_mounts_at_once() {
    let mut app = machine();
    let i = side(&app, "/dev/sdd2");
    app.open_side_item(i, false, Instant::now());
    app.jobs.form.as_mut().unwrap().set_text(1, "typed");
    app.form_submit(1);
    assert!(app.jobs.form.is_none() && launches(&mut app).is_empty());
    let i = side(&app, "/dev/sdc1");
    app.open_side_item(i, false, Instant::now());
    assert!(app.jobs.form.is_none());
    assert_eq!(launches(&mut app), [Launch::Mount(p("/dev/sdc1"))]);
}

#[test]
fn a_drive_card_on_home_mounts_an_unmounted_volume_instead_of_opening_its_device_file() {
    let mut app = machine();
    app.frame(&mut crate::text::Gfx::new(crate::text::tests::Mono, 1024), Instant::now());
    app.drain_effects().for_each(drop);
    let card = |app: &App, path: &str| app.nav.home.cards.iter().position(|(_, c)| matches!(c, super::super::home::Card::Drive(d) if app.nav.sidebar.places.drives[*d].path == p(path))).unwrap_or_else(|| panic!("no card for {path}"));
    let k = card(&app, "/dev/sdc1");
    app.open_home_item(k, false, Instant::now());
    assert_eq!(launches(&mut app), [Launch::Mount(p("/dev/sdc1"))]);
    assert!(app.tab().location().is_home_page());
    let k = card(&app, "/media/ann/STICK");
    app.open_home_item(k, false, Instant::now());
    assert!(launches(&mut app).is_empty());
    assert_eq!(app.tab().location().path(), &p("/media/ann/STICK"));
}

fn computer(app: &mut App, names: &[&str]) {
    use crate::listing::{attr, DirSnapshot, Listing, Meta};
    use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
    app.navigate(Location::computer(), Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Sensitive);
    for n in names {
        let i = listing.push(n.as_bytes(), Meta { attrs: attr::DIR | attr::VOLUME, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: std::time::Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
}

fn pick(app: &mut App, name: &str) -> u32 {
    let v = app.tabs[app.cur].view.as_mut().unwrap();
    let i = v.find(name.as_bytes()).unwrap();
    v.clear_selection();
    v.set_selected(i, true);
    v.focus = Some(i);
    i
}

#[test]
fn this_computer_mounts_an_unmounted_volume_and_offers_mount_unmount_and_eject() {
    let mut app = machine();
    computer(&mut app, &["/", "/media/ann/STICK", "/dev/sdc1", "/dev/sdd2"]);
    let verbs = |app: &mut App, name: &str| {
        pick(app, name);
        labels(&app.collect(|a, m| a.computer_drive_items(m)))
    };
    assert_eq!(verbs(&mut app, "/media/ann/STICK"), [S::MenuEject.t(), S::MenuUnmount.t()]);
    assert_eq!(verbs(&mut app, "/dev/sdc1"), [S::MenuMount.t()]);
    assert_eq!(verbs(&mut app, "/dev/sdd2"), [S::SidebarDriveUnlock.t()]);
    assert!(verbs(&mut app, "/").is_empty());
    let i = pick(&mut app, "/dev/sdc1");
    app.open_entry(i, Instant::now());
    assert_eq!(launches(&mut app), [Launch::Mount(p("/dev/sdc1"))]);
    assert!(app.tab().location().path().kind() == PathKind::Computer);
    let i = pick(&mut app, "/media/ann/STICK");
    app.open_entry(i, Instant::now());
    assert!(launches(&mut app).is_empty());
    assert_eq!(app.tab().location().path(), &p("/media/ann/STICK"));
}
