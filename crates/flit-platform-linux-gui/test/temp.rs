use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static TOOL_PATH: std::cell::RefCell<Option<std::ffi::OsString>> = const { std::cell::RefCell::new(None) };
}

pub fn tool_path() -> Option<std::ffi::OsString> {
    TOOL_PATH.with(|p| p.borrow().clone())
}

pub fn assert_missing(message: &str, tool: &str) {
    let package = crate::tools::package_for(tool, &crate::tools::os_release()).unwrap();
    assert!(message.contains(&format!("Install the package {package}")), "{message}");
    assert!(!message.contains("{package}"));
}

pub fn with_tools<T>(names: &[&str], run: impl FnOnce() -> T) -> T {
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TOOL_PATH.with(|p| *p.borrow_mut() = self.0.take());
        }
    }
    let temp = Temp::new("tools-path");
    for name in names {
        std::os::unix::fs::symlink("/bin/true", temp.path().join(name)).unwrap();
    }
    let _restore = Restore(TOOL_PATH.with(|p| p.replace(Some(temp.path().as_os_str().to_owned()))));
    run()
}

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
    for var in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS"] {
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
