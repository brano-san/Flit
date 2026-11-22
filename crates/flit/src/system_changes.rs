use crate::system_journal::{Group, Journal, Native};
use flit_core::i18n::S;
use flit_platform::{ProfilePlatform, SystemPlatform};
use std::io;

pub enum Action {
    Refresh,
    Undo,
    Autostart { on: bool, startup: bool, background: bool },
    RegisterArchive { startup: bool },
    ArchiveOfferShown,
    Protocol { on: bool, startup: bool },
}

pub struct Reply {
    pub changes: flit_core::ui::SystemChanges,
    pub result: Result<(), String>,
    pub undone: bool,
    pub disabled_autostart: bool,
    pub disabled_protocol: bool,
    pub disabled_portal: bool,
    pub offer_archives: bool,
    pub archive_registered: bool,
}

pub fn journal<P: ProfilePlatform>() -> io::Result<Journal> {
    let dir = P::state_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, S::SettingsPageSystemStateMissing.t()))?;
    Ok(Journal::new(&dir))
}

pub fn run<P: ProfilePlatform>(action: Action) -> Reply {
    let undone = matches!(action, Action::Undo);
    let archive_registered = matches!(action, Action::RegisterArchive { startup: false });
    let mut offer_archives = false;
    let journal = match journal::<P>() {
        Ok(journal) => journal,
        Err(error) => return Reply { changes: Err(error.to_string()), result: Err(error.to_string()), undone, disabled_autostart: false, disabled_protocol: false, disabled_portal: false, offer_archives, archive_registered },
    };
    let result = match action {
        Action::Refresh => Ok(()),
        Action::Undo => undo_all(&journal),
        Action::Autostart { on: false, .. } => journal.undo(&Native, Some(Group::Autostart)),
        Action::Autostart { on: true, startup, background } => apply_autostart(&journal, startup, background),
        Action::RegisterArchive { startup } => apply_archive(&journal, startup, &mut offer_archives),
        Action::ArchiveOfferShown => journal.mark_archive_offer_shown(),
        Action::Protocol { on: false, .. } => journal.undo(&Native, Some(Group::Protocol)),
        Action::Protocol { on: true, startup } => apply_protocol(&journal, startup),
    };
    #[cfg(windows)]
    if undone || archive_registered || matches!(action, Action::Protocol { .. }) {
        notify_associations();
    }
    let record = journal.inspect();
    let suppressed = |group| record.as_ref().is_ok_and(|r| !r.allows_startup(group));
    let disabled_autostart = (undone || matches!(action, Action::Autostart { startup: true, .. })) && suppressed(Group::Autostart);
    let disabled_protocol = (undone || matches!(action, Action::Protocol { startup: true, .. })) && suppressed(Group::Protocol);
    let disabled_portal = undone && suppressed(Group::Portal);
    let changes = record.map(|record| {
        let mut seen = std::collections::HashSet::new();
        record
            .changes
            .iter()
            .filter(|change| seen.insert(&change.target))
            .map(|change| {
                let title = match change.group {
                    Group::Autostart => S::SettingsPageSystemAutostart,
                    Group::Archive => S::SettingsPageSystemArchive,
                    Group::Protocol => S::SettingsPageSystemProtocol,
                    Group::Portal => S::PickPortalTitle,
                };
                (title, change.target.label())
            })
            .collect()
    });
    Reply { changes: changes.map_err(|e| e.to_string()), result: result.map_err(|e| e.to_string()), undone, disabled_autostart, disabled_protocol, disabled_portal, offer_archives, archive_registered }
}

fn undo_all(journal: &Journal) -> io::Result<()> {
    journal.undo(&Native, None)?;
    #[cfg(target_os = "linux")]
    if let Ok(path) = crate::portal_config::path() {
        crate::portal_config::strip(&path)?;
    }
    Ok(())
}

#[cfg(any(not(windows), test))]
fn launch_path<'a>(exe: &'a std::path::Path, appimage: Option<&'a std::ffi::OsStr>, appdir: Option<&std::ffi::OsStr>) -> &'a std::path::Path {
    crate::install_format::appimage_path(exe, appimage, appdir).unwrap_or(exe)
}

fn integration_exe() -> io::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    #[cfg(not(windows))]
    {
        let image = std::env::var_os("APPIMAGE");
        let dir = std::env::var_os("APPDIR");
        Ok(launch_path(&exe, image.as_deref(), dir.as_deref()).to_path_buf())
    }
    #[cfg(windows)]
    Ok(exe)
}

fn apply_autostart(journal: &Journal, startup: bool, background: bool) -> io::Result<()> {
    if crate::update::store_managed() {
        return Ok(());
    }
    let exe = integration_exe()?;
    let exe = exe.to_str().ok_or(io::ErrorKind::InvalidData)?;
    #[cfg(windows)]
    let desired = [autostart_registry(exe, background)];
    #[cfg(not(windows))]
    let desired = [(crate::system_journal::Target::File(flit_platform_linux_gui::xdg::config_home().ok_or(io::ErrorKind::NotFound)?.join("autostart/flit.desktop")), crate::system_journal::Value { kind: 0, bytes: autostart_desktop(exe, background)?.into_bytes() })];
    journal.apply(&Native, Group::Autostart, &desired, startup)
}

#[cfg(any(windows, test))]
fn reg_string(key: &str, name: &str, text: &str) -> (crate::system_journal::Target, crate::system_journal::Value) {
    use crate::system_journal::{Target, Value};
    (Target::Registry { key: key.into(), name: name.into() }, Value { kind: 1, bytes: text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect() })
}

#[cfg(any(windows, test))]
fn autostart_registry(exe: &str, background: bool) -> (crate::system_journal::Target, crate::system_journal::Value) {
    let flag = if background { " --background" } else { "" };
    reg_string(r"Software\Microsoft\Windows\CurrentVersion\Run", "Flit", &format!("\"{exe}\"{flag}"))
}

#[cfg(any(not(windows), test))]
fn autostart_desktop(exe: &str, background: bool) -> io::Result<String> {
    use flit_core::mimeapps::desktop_exec;
    let exec = desktop_exec(exe).ok_or(io::ErrorKind::InvalidInput)?;
    let exec = if background { format!("{exec} --background") } else { exec };
    let text = include_str!("../../../assets/flit.desktop").lines().take_while(|line| !line.is_empty()).filter(|line| !line.starts_with("Actions=")).map(|line| if line.starts_with("Exec=") { format!("Exec={exec}\n") } else { format!("{line}\n") }).collect::<String>();
    Ok(format!("{text}X-GNOME-Autostart-enabled=true\n"))
}

#[cfg(windows)]
fn apply_archive(journal: &Journal, startup: bool, offer: &mut bool) -> io::Result<()> {
    if crate::update::store_managed() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let exe = exe.to_str().ok_or(io::ErrorKind::InvalidData)?;
    let entries = archive_entries(exe, archive_handler);
    if entries.is_empty() {
        return if startup { Ok(()) } else { Err(io::Error::other(S::SettingsPageSystemArchiveNoHandlers.t())) };
    }
    journal.apply(&Native, Group::Archive, &entries, startup)?;
    if startup {
        *offer = journal.offer_archive_registration()?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn apply_archive(_: &Journal, _: bool, _: &mut bool) -> io::Result<()> {
    Ok(())
}

#[cfg(any(windows, test))]
const ARCHIVE_TYPES: [&str; 8] = ["zip", "7z", "rar", "tar", "tgz", "gz", "lzh", "jar"];

#[cfg(any(windows, test))]
fn archive_entries(exe: &str, mut handler: impl FnMut(&str) -> Option<String>) -> Vec<(crate::system_journal::Target, crate::system_journal::Value)> {
    let open_with: Vec<_> = ARCHIVE_TYPES
        .into_iter()
        .filter_map(|ext| {
            let extension = format!(".{ext}");
            let progid = handler(&extension)?;
            if progid.is_empty() || progid.eq_ignore_ascii_case("Flit.Archive") || progid.eq_ignore_ascii_case(r"Applications\flit.exe") {
                return None;
            }
            Some(reg_string(&format!(r"Software\Classes\{extension}\OpenWithProgids"), "Flit.Archive", ""))
        })
        .collect();
    if open_with.is_empty() {
        return Vec::new();
    }
    const ROOT: &str = r"Software\Classes\Flit.Archive";
    let mut entries = vec![reg_string(ROOT, "", "Flit"), reg_string(&format!(r"{ROOT}\DefaultIcon"), "", &format!("\"{exe}\",0")), reg_string(&format!(r"{ROOT}\shell\open\command"), "", &format!("\"{exe}\" \"%1\""))];
    entries.extend(open_with);
    entries
}

#[cfg(windows)]
fn archive_handler(extension: &str) -> Option<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCF_NOFIXUPS, ASSOCSTR_COMMAND, ASSOCSTR_DELEGATEEXECUTE, ASSOCSTR_PROGID};

    let extension: Vec<_> = extension.encode_utf16().chain([0]).collect();
    let query = |kind| {
        let mut output = [0u16; 2048];
        let mut len = output.len() as u32;
        let flags = ASSOCF_INIT_IGNOREUNKNOWN | ASSOCF_NOFIXUPS;
        let result = unsafe { AssocQueryStringW(flags, kind, PCWSTR(extension.as_ptr()), PCWSTR::null(), Some(PWSTR(output.as_mut_ptr())), &mut len) };
        let end = output.iter().position(|&ch| ch == 0)?;
        (result.is_ok() && end > 0).then(|| String::from_utf16_lossy(&output[..end]))
    };
    let progid = query(ASSOCSTR_PROGID)?;
    (query(ASSOCSTR_COMMAND).is_some() || query(ASSOCSTR_DELEGATEEXECUTE).is_some()).then_some(progid)
}

fn apply_protocol(journal: &Journal, startup: bool) -> io::Result<()> {
    if crate::update::store_managed() {
        return Ok(());
    }
    let exe = integration_exe()?;
    let exe = exe.to_str().ok_or(io::ErrorKind::InvalidData)?;
    #[cfg(windows)]
    let desired = protocol_registry(exe);
    #[cfg(not(windows))]
    let desired = {
        use crate::system_journal::{Target, Value};
        use flit_platform_linux_gui::xdg;
        let config = xdg::config_home().ok_or(io::ErrorKind::NotFound)?;
        let data = xdg::data_home().ok_or(io::ErrorKind::NotFound)?;
        [(Target::File(data.join("applications/flit-uri.desktop")), Value { kind: 0, bytes: protocol_desktop(exe)?.into_bytes() }), (Target::MimeDefault(config.join("mimeapps.list")), Value { kind: 0, bytes: b"flit-uri.desktop;".to_vec() })]
    };
    journal.apply(&Native, Group::Protocol, &desired, startup)
}

#[cfg(any(windows, test))]
fn protocol_registry(exe: &str) -> [(crate::system_journal::Target, crate::system_journal::Value); 3] {
    const ROOT: &str = r"Software\Classes\flit";
    [reg_string(ROOT, "", "URL:Flit"), reg_string(ROOT, "URL Protocol", ""), reg_string(&format!(r"{ROOT}\shell\open\command"), "", &format!("\"{exe}\" {} \"%1\"", crate::system_link::HANDLER))]
}

#[cfg(any(not(windows), test))]
fn protocol_desktop(exe: &str) -> io::Result<String> {
    let exec = flit_core::mimeapps::desktop_exec(exe).ok_or(io::ErrorKind::InvalidInput)?;
    let handler = crate::system_link::HANDLER;
    Ok(format!("[Desktop Entry]\nType=Application\nName=Flit\nIcon=flit\nNoDisplay=true\nExec={exec} {handler} %u\nMimeType=x-scheme-handler/flit;\n"))
}

#[cfg(windows)]
fn notify_associations() {
    use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

pub fn unregister<P: ProfilePlatform + SystemPlatform>() -> i32 {
    #[cfg(windows)]
    flit_platform_windows::diagnose::attach_console();
    let ui = std::env::var("FLIT_LANG").unwrap_or_else(|_| P::ui_language());
    let lang = flit_core::i18n::Lang::from_code(&ui).unwrap_or_default();
    flit_core::i18n::set_lang(lang);
    let result = journal::<P>().and_then(|journal| undo_all(&journal));
    #[cfg(windows)]
    notify_associations();
    match result {
        Ok(()) => {
            println!("{}", S::SettingsPageSystemUndone.t());
            0
        }
        Err(error) => {
            eprintln!("{}", flit_core::tr!(S::SettingsPageSystemWriteFailed, error = error));
            1
        }
    }
}

#[cfg(test)]
#[path = "../test/system_changes.rs"]
mod tests;
