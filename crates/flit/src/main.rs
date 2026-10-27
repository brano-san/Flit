#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod diagnose;
mod find;
mod git;
mod logs;
mod metrics;
mod places;
mod preview;
mod schema;
mod session;
mod settings;
mod search_client;
mod state;
mod tags;
mod thumbs;

#[cfg(not(target_os = "linux"))]
use flit_core::open::parse_args;
#[cfg(target_os = "linux")]
use flit_platform_linux_gui::instance::parse_args;
use flit_platform::{Instance, Platform, SingleInstance};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Instant;

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

fn main() {
    let started = Instant::now();
    let mut args: Vec<Vec<u8>> = Vec::new();
    let mut metrics: Option<PathBuf> = None;
    let mut activated = false;
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        if a == "--diagnose" {
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
        } else if a == "--dbus-activated" {
            activated = true;
        } else if a == "--metrics" {
            metrics = it.next().map(PathBuf::from);
        } else {
            args.push(wtf8(&a));
        }
    }
    let cwd = std::env::current_dir().ok().map(|d| wtf8(d.as_os_str()));
    let open = parse_args(&args, cwd);
    let mut standalone = None;
    let instance = match metrics {
        Some(_) => None,
        None => match <P as Platform>::Instance::acquire(&open) {
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
    let logs_dir = <P as Platform>::state_dir().map(|d| d.join("logs"));
    logs::init(logs_dir.clone(), std::env::var_os("FLIT_LOG").is_some());
    #[cfg(windows)]
    if let Some(dir) = &logs_dir {
        flit_platform_windows::crash::install(dir);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            previous(info);
            flit_platform_windows::crash::dump_now();
        }));
    }
    tracing::info!("Flit {} started, process {}", env!("CARGO_PKG_VERSION"), std::process::id());
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
    if let Err(e) = app::run::<P>(open, instance, metrics, activated, started) {
        tracing::error!("cannot start: {e}");
        logs::flush();
        if headless {
            eprintln!("flit: {e}");
        } else {
            <P as Platform>::fatal_error(&flit_core::tr!(flit_core::i18n::S::UiAppStartFailed, error = flit_core::i18n::localize(&e.to_string())));
        }
        std::process::exit(1);
    }
    tracing::info!("Flit exits");
    logs::flush();
}
