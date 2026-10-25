use flit_platform::{Platform, PlatformError, Service};

#[cfg(windows)]
type P = flit_platform_windows::Windows;
#[cfg(not(windows))]
type P = flit_platform_linux::Linux;

type S = <P as Platform>::Service;

fn body() -> Result<(), PlatformError> {
    println!("flit-indexd: endpoint {}, protocol v{}", S::endpoint(), flit_search::proto::VERSION);
    Ok(())
}

fn main() {
    if std::env::args().skip(1).any(|a| a == "--version" || a == "-V") {
        println!("flit-indexd {} (protocol v{})", env!("CARGO_PKG_VERSION"), flit_search::proto::VERSION);
        return;
    }
    if let Err(e) = S::run(body) {
        eprintln!("flit-indexd: {e}");
        std::process::exit(1);
    }
}
