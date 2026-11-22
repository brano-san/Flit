use crate::{AgentImages, AgentLauncher, Awake, FileOps, Fs, IndexSource, PlatformError, Renderer, Resolved, Service, Shell, SingleInstance, Text, Watcher, WindowSystem};
use flit_core::tags::FileKey;
use flit_core::ui::NewTemplate;
use flit_core::{FsPath, Location};

pub trait WindowPlatform {
    type WindowSystem: WindowSystem;
    type Renderer: Renderer<Window = <Self::WindowSystem as WindowSystem>::Window>;
    type Text: Text;

    fn text() -> Result<Self::Text, PlatformError>;

    fn dark_mode() -> bool;

    fn fatal_error(message: &str);

    fn refresh_period() -> Option<std::time::Duration>;

    fn double_click_time() -> std::time::Duration {
        std::time::Duration::from_millis(500)
    }

    fn wheel_lines() -> u32 {
        3
    }
}

pub trait FilePlatform {
    type Fs: Fs;
    type Watcher: Watcher;

    fn fs() -> Self::Fs;

    fn caps() -> flit_core::caps::Caps {
        flit_core::caps::Caps::default()
    }

    fn recycle_bin() -> bool {
        false
    }

    fn file_key(_path: &FsPath) -> Option<FileKey> {
        None
    }

    fn resolve_files(files: &[(FileKey, &FsPath)]) -> Vec<Resolved> {
        files.iter().map(|_| Resolved::Unavailable).collect()
    }

    fn sync_tag_names(_path: &FsPath, _drop: &[String], _keep: &[String]) {}

    fn raise_watch_limit() -> flit_core::ui::WatchLimit {
        flit_core::ui::WatchLimit::Failed(PlatformError::unsupported("raising the watch limit").message)
    }
}

pub trait ShellPlatform {
    type Shell: Shell + FileOps;

    fn home() -> FsPath;

    fn known_folder(name: &str) -> Option<Location>;

    fn business_onedrive() -> Vec<(String, Location)> {
        Vec::new()
    }

    fn templates() -> Vec<NewTemplate> {
        Vec::new()
    }

    fn libraries() -> Vec<flit_core::library::Library> {
        Vec::new()
    }

    fn library(_op: &flit_core::library::LibraryOp) -> Result<(), PlatformError> {
        Err(PlatformError::new(-1, flit_core::i18n::S::ErrorsEngineNoLibraries.en()))
    }

    fn recent(_limit: usize) -> (Vec<flit_core::ui::Recent>, bool) {
        (Vec::new(), false)
    }

    fn bookmarks() -> Vec<Location> {
        Vec::new()
    }

    fn bookmarks_changed(_added: &[Location], _removed: &[Location]) {}

    fn jump_list(_pinned: &[Location], _frequent: &[Location]) {}

    fn quick_access() -> Option<Box<dyn flit_core::quick_sync::QuickAccess>> {
        None
    }

    fn never_shown_extensions() -> Vec<String> {
        Vec::new()
    }
}

pub trait DevicePlatform {
    fn drives() -> Vec<flit_core::ui::Drive>;

    fn wsl_distros() -> Vec<String>;

    fn watch_wsl(_changed: Box<dyn Fn() + Send>) {}

    fn wsl_launcher(_distro: &str, _images: &AgentImages) -> Option<Box<dyn AgentLauncher>> {
        None
    }
}

pub trait ProfilePlatform {
    fn state_dir() -> Option<std::path::PathBuf>;

    fn cache_dir() -> Option<std::path::PathBuf> {
        Self::state_dir()
    }

    fn config_dir() -> Option<std::path::PathBuf> {
        Self::state_dir()
    }
}

pub trait SystemPlatform {
    type Instance: SingleInstance;

    fn utc_offset_minutes() -> i32;

    fn elevated() -> bool {
        false
    }

    fn date_patterns() -> (String, String) {
        ("yyyy-MM-dd".into(), "HH:mm".into())
    }

    fn os_version() -> String {
        std::env::consts::OS.to_owned()
    }

    fn missing_tools() -> Vec<flit_core::ui::MissingTool> {
        Vec::new()
    }

    fn ui_language() -> String {
        String::new()
    }

    fn integrate(_on: bool) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("system integration"))
    }

    fn tidy_integration() {}

    fn admin_launcher() -> Option<Box<dyn AgentLauncher>> {
        None
    }

    fn wait_exit(_pid: u32, _limit: std::time::Duration) {}

    fn keep_awake(_reason: &str) -> Option<Awake> {
        None
    }

    fn run_elevated(_args: &[String]) -> Result<u32, PlatformError> {
        Err(PlatformError::unsupported("running with administrator rights"))
    }
}

pub trait SearchPlatform {
    type IndexSource: IndexSource;
    type Service: Service;
    type FallbackIndex: IndexSource + 'static;

    fn shell_search(_folder: Option<&FsPath>, _query: &str, _keep_going: &dyn Fn() -> bool, _found: &mut dyn FnMut(FsPath, flit_core::Meta)) -> Result<(), PlatformError> {
        Err(PlatformError::new(-1, flit_core::i18n::S::ErrorsEngineNoWindowsSearch.en()))
    }
}

pub trait NetworkPlatform {
    fn network_warning() -> Option<flit_core::provider::NetworkWarning> {
        None
    }

    fn download(_url: &str, _out: &mut dyn std::io::Write, _progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("downloads"))
    }
}

pub trait Platform:
    WindowPlatform + FilePlatform + ShellPlatform + DevicePlatform
    + ProfilePlatform + SystemPlatform + SearchPlatform + NetworkPlatform + 'static
{
}
