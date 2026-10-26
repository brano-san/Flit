use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

#[used]
#[link_section = ".init_array"]
static ISOLATE: extern "C" fn() = isolate;

extern "C" fn isolate() {
    let root = PathBuf::from(format!("/tmp/flit-test-{}", unsafe { libc::getuid() }));
    let (home, runtime) = (root.join("home"), root.join("runtime"));
    let _ = std::fs::create_dir_all(&home);
    let _ = std::fs::create_dir_all(&runtime);
    let _ = std::fs::set_permissions(&runtime, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    for var in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "WAYLAND_DISPLAY"] {
        std::env::remove_var(var);
    }
    if std::env::var("DISPLAY").is_ok_and(|d| d == ":0" || d.starts_with(":0.")) {
        std::env::remove_var("DISPLAY");
    }
}

pub struct Temp(pub PathBuf);

impl Temp {
    pub fn new(tag: &str) -> Self {
        let dir = PathBuf::from(format!("/tmp/flit-gui-test-{tag}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn file(&self, name: &str, bytes: usize) -> PathBuf {
        let p = self.0.join(name);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(&p, vec![b'x'; bytes]).unwrap();
        p
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
