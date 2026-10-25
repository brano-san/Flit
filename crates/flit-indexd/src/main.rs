mod serve;

use flit_platform::{start_indexer, IndexSource, Platform, PlatformError, Service};
use flit_search::proto::VolumeState;
use flit_search::session::SharedIndex;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[cfg(windows)]
type P = flit_platform_windows::Windows;
#[cfg(not(windows))]
type P = flit_platform_linux::Linux;

type S = <P as Platform>::Service;

const USAGE: &str = "usage: flit-indexd [--console] [--walk [ROOT...]] [--snapshots DIR] [--query TEXT] [--version]

  (no options)  run as the Windows service (started by the service control manager)
  --console     run in the foreground and print index progress
  --query TEXT  ask the running service and print the first results
  --walk        index by walking folders (no administrator rights needed) instead of the MFT;
                without ROOTs every fixed drive is walked
  --snapshots DIR  keep index snapshots for fast restarts in DIR
                (default: %LOCALAPPDATA%\\Flit\\indexd of the account the service runs as)";

#[derive(Default)]
struct Config {
    console: bool,
    walk: Option<Vec<String>>,
    query: Option<String>,
    snapshots: Option<std::path::PathBuf>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

#[cfg(windows)]
fn walk_source(roots: &[String]) -> Result<<P as Platform>::FallbackIndex, PlatformError> {
    if roots.is_empty() {
        return <P as Platform>::FallbackIndex::new();
    }
    let parsed: Option<Vec<_>> = roots.iter().map(|r| flit_core::FsPath::parse(r)).collect();
    let parsed = parsed.ok_or_else(|| PlatformError::new(-1, "a --walk root is not an absolute path"))?;
    Ok(flit_platform_windows::WalkIndex::with_roots(parsed))
}

#[cfg(not(windows))]
fn walk_source(_: &[String]) -> Result<<P as Platform>::FallbackIndex, PlatformError> {
    <P as Platform>::FallbackIndex::new()
}

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

fn body() -> Result<(), PlatformError> {
    let cfg = CONFIG.get_or_init(Config::default);
    let shared = SharedIndex::new();
    let snapshots = cfg.snapshots.clone().or_else(|| P::state_dir().map(|d| d.join("indexd")));
    match &cfg.walk {
        Some(roots) => start_indexer(walk_source(roots)?, shared.clone(), snapshots)?,
        None => start_indexer(<P as Platform>::IndexSource::new()?, shared.clone(), snapshots)?,
    }
    if cfg.console {
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("flit-indexd-report".into()).spawn(move || report(s));
        println!("flit-indexd: serving {} (protocol v{})", S::endpoint(), flit_search::proto::VERSION);
    }
    let utc = P::utc_offset_minutes();
    S::listen(
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

fn query(text: &str) -> Result<(), String> {
    use flit_platform::Duplex;
    use flit_proto::transport::{handshake, read_frame, write_frame, Hello, Protocol};
    use flit_search::proto::{Message, Sort, SortKey, VERSION};
    let mut conn = S::connect().map_err(|e| e.to_string())?;
    conn.set_read_timeout(Some(Duration::from_secs(10)));
    handshake(&mut conn, &Hello::new(Protocol::Index, VERSION)).map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    write_frame(&mut conn, &Message::Query { gen: 1, text: text.into(), sort: Sort { key: SortKey::Name, descending: false } }).map_err(|e| e.to_string())?;
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
                    let sep = if dir.ends_with(['\\', '/']) { "" } else { "\\" };
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
            root if !root.starts_with("--") && cfg.walk.is_some() => cfg.walk.as_mut().unwrap().push(root.to_owned()),
            other => {
                eprintln!("flit-indexd: unknown option {other}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    if let Some(text) = &cfg.query {
        if let Err(e) = query(text) {
            eprintln!("flit-indexd: {e}");
            std::process::exit(1);
        }
        return;
    }
    let console = cfg.console;
    let _ = CONFIG.set(cfg);
    let result = if console { body() } else { S::run(body) };
    if let Err(e) = result {
        eprintln!("flit-indexd: {e}");
        if !console {
            eprintln!("{USAGE}");
        }
        std::process::exit(1);
    }
}
