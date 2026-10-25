#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod find;
mod metrics;
mod places;
mod preview;
mod session;
mod search_client;
mod state;
mod tags;
mod thumbs;

use flit_core::name;
use flit_core::open::parse_args;
use flit_platform::{Instance, Platform, SingleInstance};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Instant;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(windows)]
pub type P = flit_platform_windows::Windows;
#[cfg(not(windows))]
pub type P = flit_platform_linux::Linux;

fn wtf8(s: &OsStr) -> Vec<u8> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let w: Vec<u16> = s.encode_wide().collect();
        let mut out = Vec::with_capacity(w.len());
        name::push_utf16(&mut out, &w);
        out
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
}

fn init_logging() {
    #[cfg(feature = "profile")]
    {
        use tracing_subscriber::layer::SubscriberExt;
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry().with(tracing_tracy::TracyLayer::default()));
        return;
    }
    #[allow(unreachable_code)]
    if std::env::var_os("FLIT_LOG").is_some() {
        let _ = tracing_subscriber::fmt().with_writer(std::io::stderr).try_init();
    }
}

fn main() {
    let started = Instant::now();
    let mut args: Vec<Vec<u8>> = Vec::new();
    let mut metrics: Option<PathBuf> = None;
    let mut it = std::env::args_os().skip(1);
    while let Some(a) = it.next() {
        if a == "--metrics" {
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
    init_logging();
    let headless = metrics.is_some();
    if let Some(e) = standalone {
        tracing::warn!("running without single-instance forwarding: {e}");
    }
    if open.tag.is_some() {
        if let Err(e) = tags::run_command::<P>(&open) {
            <P as Platform>::fatal_error(&format!("Flit could not tag the files: {e}"));
        }
        return;
    }
    if let Err(e) = app::run::<P>(open, instance, metrics, started) {
        tracing::error!("{e}");
        if headless {
            eprintln!("flit: {e}");
        } else {
            <P as Platform>::fatal_error(&format!("Flit could not start: {e}"));
        }
        std::process::exit(1);
    }
}
