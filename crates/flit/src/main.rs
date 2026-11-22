#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod diagnose;
mod find;
mod git;
mod jumps;
mod layouts;
mod logs;
mod portable_meta;
mod disk_changes;
mod metrics;
mod overlays;
mod parser;
mod places;
mod quick_sync;
mod preview;
mod schema;
mod session;
mod settings;
mod search_client;
mod state;
mod tags;
mod update;
mod thumbs;
mod system_changes;
mod system_journal;
mod system_link;
#[cfg(any(not(windows), test))]
mod install_format;
mod picker;
#[cfg(any(test, target_os = "linux"))]
mod portal_config;

#[cfg(not(target_os = "linux"))]
use flit_core::open::parse_args;
#[cfg(target_os = "linux")]
use flit_platform_linux_gui::instance::parse_args;
use flit_platform::{Instance, Platform, SingleInstance};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Instant;

const RESTARTED_BY_WINDOWS: &str = "--restarted-by-windows";

#[cfg(windows)]
pub type P = flit_platform_windows::Windows;
#[cfg(not(windows))]
pub type P = flit_platform_linux_gui::Linux;

fn wtf8(s: &OsStr) -> Vec<u8> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let w: Vec<u16> = s.encode_wide().collect();
        let mut out = Vec::with_capacity(w.len());
        flit_core::name::push_utf16(&mut out, &w);
        out
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
}

#[cfg(windows)]
fn hand_to_explorer(open: &flit_core::open::Open) -> bool {
    let locations = flit_core::open::explorer_only(open, &P::known_folder);
    for location in locations.iter().filter(|l| !flit_platform_windows::raise_explorer(l)) {
        let _ = std::process::Command::new("explorer.exe").arg(location).spawn();
    }
    !locations.is_empty()
}

fn main() {
    let started = Instant::now();
    let mut args: Vec<Vec<u8>> = Vec::new();
    let mut metrics: Option<PathBuf> = None;
    let mut activated = false;
    let mut background = false;
    let raw: Vec<_> = std::env::args_os().skip(1).collect();
    let link = match system_link::cli(&raw) {
        Some(Ok(open)) => Some(open),
        Some(Err(error)) => {
            let lang = std::env::var("FLIT_LANG").unwrap_or_else(|_| P::ui_language());
            flit_core::i18n::set_lang(flit_core::i18n::Lang::from_code(&lang).unwrap_or_default());
            P::fatal_error(error.t());
            std::process::exit(2);
        }
        None => None,
    };
    let mut it = if link.is_some() { Vec::new() } else { raw }.into_iter();
    while let Some(a) = it.next() {
        if a == flit_platform::parse_process::SUBCOMMAND {
            std::process::exit(parser::serve(it));
        } else if a == preview::pretty::COMMAND {
            std::process::exit(preview::pretty::serve(it.next().as_deref()));
        } else if a == preview::svg::COMMAND {
            std::process::exit(preview::svg::serve(it.next().as_deref(), it.next().as_deref()));
        } else if a == "--diagnose" {
            #[cfg(windows)]
            flit_platform_windows::diagnose::attach_console();
            match diagnose::run::<P>() {
                Ok(path) => println!("{}", path.display()),
                Err(e) => {
                    eprintln!("flit: cannot write the diagnostics file: {e}");
                    std::process::exit(1);
                }
            }
            return;
        } else if a == "--elevated-agent" {
            #[cfg(target_os = "linux")]
            {
                use flit_platform_linux_gui::elevated;
                let lang = it.next();
                let companions = it.next().is_some_and(|arg| arg == elevated::COMPANIONS_FLAG);
                std::process::exit(elevated::serve(lang.as_deref(), companions));
            }
        } else if a == "--create-symbolic-link" {
            #[cfg(windows)]
            std::process::exit(flit_platform_windows::symbolic_from_command_line(it));
        } else if a == "--label-volume" {
            #[cfg(windows)]
            std::process::exit(flit_platform_windows::label_from_command_line(it.next(), it.next()));
        } else if a == flit_core::disk_changes::ELEVATED_FLAG {
            #[cfg(windows)]
            std::process::exit(flit_platform_windows::disk_changes::from_command_line(it));
        } else if a == update::INSTALL_ELEVATED {
            std::process::exit(update::install_from_command_line(it.next(), it.next()));
        } else if a == "--dbus-activated" {
            activated = true;
        } else if a == "--metrics" {
            metrics = it.next().map(PathBuf::from);
        } else if a == RESTARTED_BY_WINDOWS {
            update::resume_session();
        } else if a == "--unregister" {
            std::process::exit(system_changes::unregister::<P>());
        } else if a == "--background" {
            background = true;
        } else if a == "--pick" {
            let request = flit_core::pick::Request::from_cli(it.map(|a| wtf8(&a)).collect());
            std::process::exit(picker::cli(request, started));
        } else if a == "--file-chooser-portal" {
            #[cfg(target_os = "linux")]
            std::process::exit(flit_platform_linux_gui::file_chooser::run());
        } else if a == "--file-chooser-request" {
            #[cfg(target_os = "linux")]
            std::process::exit(picker::portal_request(started));
        } else if cfg!(target_os = "linux") && a == "--restore-file-chooser" {
            #[cfg(target_os = "linux")]
            if let Err(error) = portal_config::restore::<P>() {
                use flit_core::i18n::{self, S};
                picker::language();
                eprintln!("{}", flit_core::tr!(S::PickPortalFailed, error = i18n::localize(&error.to_string())));
                std::process::exit(1);
            }
            return;
        } else {
            args.push(wtf8(&a));
        }
    }
    let cwd = std::env::current_dir().ok().map(|d| wtf8(d.as_os_str()));
    let open = link.unwrap_or_else(|| parse_args(&args, cwd));
    #[cfg(windows)]
    if !system_link::handles(&open) && hand_to_explorer(&open) {
        return;
    }
    update::on_start::<P>();
    let mut standalone = None;
    let background = background && open.is_empty() && open.tag.is_none() && metrics.is_none() && !activated;
    let instance = match metrics {
        Some(_) => None,
        None => match if background { <P as Platform>::Instance::acquire_background() } else { <P as Platform>::Instance::acquire(&open) } {
            Instance::Forwarded => {
                if std::env::var_os("FLIT_TIMING").is_some() {
                    eprintln!("forwarded in {:?}", started.elapsed());
                }
                return;
            }
            Instance::Primary(i) => Some(i),
            Instance::Standalone(e) => {
                standalone = Some(e);
                None
            }
        },
    };
    #[cfg(windows)]
    let identity = flit_platform_windows::aumid::adopt();
    let logs_dir = <P as Platform>::state_dir().map(|d| d.join("logs"));
    #[cfg(windows)]
    if let Some(dir) = &logs_dir {
        flit_platform_windows::crash::install(dir);
    }
    logs::init(logs_dir, std::env::var_os("FLIT_LOG").is_some());
    #[cfg(windows)]
    if metrics.is_none() {
        flit_platform_windows::crash::register_restart(RESTARTED_BY_WINDOWS);
    }
    tracing::info!("Flit {} started, process {}", env!("CARGO_PKG_VERSION"), std::process::id());
    #[cfg(windows)]
    if let Err(e) = &identity {
        tracing::warn!("cannot set the application identity of the process: {e}");
    }
    let system = flit_core::i18n::Lang::from_code(&std::env::var("FLIT_LANG").unwrap_or_else(|_| <P as Platform>::ui_language())).unwrap_or_default();
    flit_core::i18n::set_system_lang(system);
    flit_core::i18n::set_lang(system);
    let headless = metrics.is_some();
    if let Some(e) = standalone {
        tracing::warn!("running without single-instance forwarding: {e}");
    }
    if open.tag.is_some() {
        if let Err(e) = tags::run_command::<P>(&open) {
            tracing::error!("cannot tag the files: {e}");
            logs::flush();
            <P as Platform>::fatal_error(&flit_core::tr!(flit_core::i18n::S::UiAppTagFailed, error = flit_core::i18n::localize(&e.to_string())));
        }
        return logs::flush();
    }
    #[cfg(target_os = "linux")]
    flit_platform_linux_gui::prefetch_fonts();
    #[cfg(windows)]
    if identity.is_ok() && !headless {
        flit_platform_windows::aumid::fix_shortcuts_in_background();
    }
    let _ = std::thread::Builder::new().name("flit-sweep".into()).spawn(|| {
        let gone = flit_core::archive::sweep_temp(&flit_core::archive::temp_root(), flit_core::archive::TEMP_KEEP, std::time::SystemTime::now());
        if gone > 0 {
            tracing::info!("removed {gone} old unpacked copies");
        }
    });
    if let Err(e) = app::run::<P>(open, instance, metrics, activated, background, None, started) {
        let ran = app::shown();
        if ran {
            tracing::error!("stopped: {e}");
        } else {
            tracing::error!("cannot start: {e}");
        }
        logs::flush();
        if headless {
            eprintln!("flit: {e}");
        } else {
            let failed = if ran { flit_core::i18n::S::UiAppStopped } else { flit_core::i18n::S::UiAppStartFailed };
            <P as Platform>::fatal_error(&flit_core::tr!(failed, error = flit_core::i18n::localize(&e.to_string())));
        }
        std::process::exit(1);
    }
    tracing::info!("Flit exits");
    update::relaunch_if_installed();
    logs::flush();
}
