use crate::{Placement, SelectionSource, WindowId, WindowOptions};
use flit_core::input::CursorIcon;
use flit_core::preview::HostCmd;
use flit_core::render::Rect;
use flit_core::ui::{Bitmap, PrimaryPaste, ShellMenu};
use flit_core::FsPath;

pub trait WindowFrame {
    fn id(&self) -> WindowId;

    fn size(&self) -> (u32, u32);

    fn scale(&self) -> f32;

    fn request_redraw(&self);

    fn set_title(&self, title: &str);

    fn set_cursor(&self, cursor: CursorIcon);

    fn set_ime_area(&self, area: Rect);

    fn is_dark_mode(&self) -> bool;

    fn set_timer(&self, ms: u32);

    fn to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        (x as i32, y as i32)
    }

    fn window_at(&self, _x: f32, _y: f32) -> Option<(WindowId, f32, f32)> {
        None
    }

    fn set_dark(&self, _dark: bool) {}
}

pub trait WindowControls {
    fn minimize(&self);

    fn toggle_maximize(&self);

    fn is_maximized(&self) -> bool;

    fn close(&self);

    fn activate(&self);

    fn open_window(&self, opts: WindowOptions);

    fn placement(&self) -> Placement;

    fn toggle_fullscreen(&self) {}

    fn request_attention(&self) {}

    fn set_compact(&self, _on: Option<bool>) {}

    fn set_server_decorations(&self, _on: bool) {}
}

pub trait WindowTransfer {
    fn clipboard_text(&self) -> Option<String>;

    fn set_clipboard_text(&self, text: &str);

    fn set_clipboard_image(&self, _image: &Bitmap) {}

    fn set_primary_text(&self, _text: &str) {}

    fn primary_text(&self, _paste: PrimaryPaste) -> Option<String> {
        None
    }

    fn set_clipboard_files(&self, paths: &[FsPath], cut: bool);

    fn start_drag(&self, paths: Vec<FsPath>);
}

pub trait WindowShell {
    fn set_folder(&self, _folder: Option<&FsPath>) {}

    fn set_selection(&self, _selection: &dyn Fn() -> SelectionSource) -> bool {
        true
    }

    fn shell_menu(&self, menu: ShellMenu);

    fn properties(&self, paths: Vec<FsPath>);

    fn set_taskbar_progress(&self, _progress: Option<(u32, bool)>) {}

    fn notify(&self, _text: &str) {}

    fn host(&self, _cmd: HostCmd) {}

    fn share(&self, _paths: Vec<FsPath>) {}

    fn set_volume_label(&self, _drive: FsPath, _label: String) {}

    fn shell_pick(&self, _cmd: Option<u32>) {}

    fn palette_shell(&self, _ticket: u64, _menu: ShellMenu) {}

    fn palette_shell_pick(&self, _ticket: u64, _command: Option<u32>) {}

    fn shell_prefetch(&self, _menu: Option<ShellMenu>) {}

    fn shell_warm(&self, _menus: Vec<ShellMenu>) {}
}

pub trait Window: WindowFrame + WindowControls + WindowTransfer + WindowShell {}
