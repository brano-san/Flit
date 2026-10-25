use flit_platform_linux::agent;
use flit_platform_linux::Trash;

fn main() {
    let code = match agent::serve(std::io::stdin().lock(), std::io::stdout(), agent::own_build(), Trash::from_env()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("flit-wsl-agent: {e}");
            1
        }
    };
    std::process::exit(code);
}
