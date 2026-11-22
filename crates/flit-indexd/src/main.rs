mod serve;
#[cfg(target_os = "linux")]
mod upkeep;

use flit_platform::{start_indexer, IndexPlatform, IndexSource, PlatformError, Service};
use flit_search::proto::VolumeState;
use flit_search::session::SharedIndex;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[cfg(windows)]
type P = flit_platform_windows::Windows;
#[cfg(not(windows))]
type P = flit_platform_linux::LinuxIndex;

type S = <P as IndexPlatform>::Service;

const USAGE: &str = "usage: flit-indexd [--console] [--walk [ROOT...]] [--snapshots DIR] [--query TEXT] [--version]

  (no options)  run as the index service: the Windows service started by the service control manager,
                or on Linux the systemd --user service serving $XDG_RUNTIME_DIR/flit/indexd.sock
  --console     run in the foreground and print index progress
  --query TEXT  ask the running service and print the first results
  --walk        index by walking folders (no administrator rights needed) instead of the MFT;
                without ROOTs every fixed drive is walked
  --snapshots DIR  keep index snapshots for fast restarts in DIR
                (default: %LOCALAPPDATA%\\Flit\\indexd of the account the service runs as,
                on Linux $XDG_STATE_HOME/flit/indexd)";

#[derive(Default)]
struct Config {
    console: bool,
    walk: Option<Vec<String>>,
    query: Option<String>,
    snapshots: Option<std::path::PathBuf>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

fn report(shared: Arc<SharedIndex>) {
    let mut seen = 0;
    loop {
        let (version, volumes) = shared.status();
        if version != seen {
            seen = version;
            for v in volumes {
                let state = match v.state {
                    VolumeState::Scanning { progress } => format!("indexing {:.0}%", progress * 100.0),
                    VolumeState::Ready => "ready".into(),
                    VolumeState::Unsupported => "not supported".into(),
                    VolumeState::Error(e) => format!("error: {e}"),
                };
                println!("flit-indexd: {} {} files, {state}", if v.id.is_empty() { "index" } else { &v.id }, v.files);
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn body<P: IndexPlatform>() -> Result<(), PlatformError> {
    let cfg = CONFIG.get_or_init(Config::default);
    let shared = SharedIndex::new();
    let snapshots = cfg.snapshots.clone().or_else(|| P::state_dir().map(|d| d.join("indexd")));
    #[cfg(target_os = "linux")]
    {
        if let Some(old) = upkeep::old_snapshots().filter(|o| cfg.snapshots.as_ref() != Some(o) && snapshots.as_ref() != Some(o)) {
            upkeep::remove_old_snapshots(&old);
        }
        upkeep::watch_binary();
    }
    match &cfg.walk {
        Some(roots) => start_indexer(P::walk_source(roots)?, shared.clone(), snapshots)?,
        None => start_indexer(P::Source::new()?, shared.clone(), snapshots)?,
    }
    if cfg.console {
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("flit-indexd-report".into()).spawn(move || report(s));
        println!("flit-indexd: serving {} (protocol v{})", P::Service::endpoint(), flit_search::proto::VERSION);
    }
    let utc = P::utc_offset_minutes();
    P::Service::listen(
        serve::MAX_CLIENTS,
        Arc::new(move |conn| {
            if let Err(e) = serve::serve(conn, shared.clone(), utc) {
                if CONFIG.get().is_some_and(|c| c.console) {
                    eprintln!("flit-indexd: client dropped: {e}");
                }
            }
        }),
    )
}

fn query<P: IndexPlatform>(text: &str) -> Result<(), String> {
    use flit_platform::Duplex;
    use flit_proto::transport::{handshake, read_frame, write_frame, Hello, Protocol};
    use flit_search::proto::{Message, Sort, SortKey, VERSION};
    let mut conn = P::Service::connect().map_err(|e| e.to_string())?;
    conn.set_read_timeout(Some(Duration::from_secs(10)));
    handshake(&mut conn, &Hello::new(Protocol::Index, VERSION)).map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    write_frame(&mut conn, &Message::Query { gen: 1, text: text.into(), conditions: Vec::new(), sort: Sort { key: SortKey::Name, descending: false }, paths: Default::default() }).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    loop {
        match read_frame::<_, Message>(&mut conn, &mut buf).map_err(|e| e.to_string())? {
            Message::QueryDone { total, .. } => {
                println!("{total} results in {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
                if total == 0 {
                    return Ok(());
                }
            }
            Message::Rows { entries, dirs, .. } => {
                for ((name, meta), dir) in entries.entries(true).map_err(|e| e.to_string())?.zip(dirs) {
                    let dir = String::from_utf8_lossy(&dir);
                    let sep = if dir.ends_with(['\\', '/']) { "" } else if cfg!(windows) { "\\" } else { "/" };
                    println!("{:>12}  {dir}{sep}{}", meta.size, String::from_utf8_lossy(name));
                }
                return Ok(());
            }
            Message::Error { message } => return Err(message),
            _ => {}
        }
    }
}

fn main() {
    let mut cfg = Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--version" | "-V" => {
                println!("flit-indexd {} (protocol v{})", env!("CARGO_PKG_VERSION"), flit_search::proto::VERSION);
                return;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            "--console" => cfg.console = true,
            "--walk" => cfg.walk = Some(Vec::new()),
            "--query" => cfg.query = Some(args.next().unwrap_or_default()),
            "--snapshots" => cfg.snapshots = args.next().map(Into::into),
            other => {
                let Some(walk) = cfg.walk.as_mut().filter(|_| !other.starts_with("--")) else {
                    eprintln!("flit-indexd: unknown option {other}\n{USAGE}");
                    std::process::exit(2);
                };
                walk.push(other.to_owned());
            }
        }
    }
    if let Some(text) = &cfg.query {
        if let Err(e) = query::<P>(text) {
            eprintln!("flit-indexd: {e}");
            std::process::exit(1);
        }
        return;
    }
    let console = cfg.console;
    let _ = CONFIG.set(cfg);
    let result = if console { body::<P>() } else { S::run(body::<P>) };
    if let Err(e) = result {
        eprintln!("flit-indexd: {e}");
        if !console {
            eprintln!("{USAGE}");
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
#[path = "../test/units.rs"]
mod units;
