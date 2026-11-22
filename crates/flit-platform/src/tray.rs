use crate::Wake;
use flit_core::Location;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub new_window: String,
    pub bookmarks: String,
    pub pinned: Vec<(Location, String)>,
    pub quit: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Activate,
    NewWindow,
    Bookmark(Location),
    Quit,
}

pub type MenuListener = Arc<dyn Fn(&Tray, bool) + Send + Sync>;

#[derive(Default)]
struct State {
    available: Option<bool>,
    menu: Option<Arc<Menu>>,
    menu_listener: Option<MenuListener>,
    actions: Vec<Action>,
}

#[derive(Clone)]
pub struct Tray {
    state: Arc<Mutex<State>>,
    wake: Wake,
}

impl Tray {
    pub fn new(wake: Wake) -> Self {
        Self { state: Arc::default(), wake }
    }

    pub fn available(&self) -> Option<bool> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).available
    }

    pub fn set_available(&self, available: bool) {
        self.set_availability(Some(available));
    }

    pub fn set_availability(&self, available: Option<bool>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.available != available {
            state.available = available;
            drop(state);
            (self.wake)();
        }
    }

    pub fn menu(&self) -> Option<Arc<Menu>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).menu.clone()
    }

    pub fn set_menu(&self, menu: Option<Menu>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.menu.as_deref() == menu.as_ref() {
            return;
        }
        let enabled = menu.is_some();
        state.menu = menu.map(Arc::new);
        let listener = state.menu_listener.clone();
        drop(state);
        if let Some(listener) = listener {
            listener(self, enabled);
        }
    }

    pub fn set_menu_listener(&self, listener: Option<MenuListener>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::mem::replace(&mut state.menu_listener, listener.clone());
        let enabled = state.menu.is_some();
        drop(state);
        if let Some(previous) = previous {
            previous(self, false);
        }
        if let Some(listener) = listener {
            listener(self, enabled);
        }
    }

    pub fn send(&self, action: Action) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).actions.push(action);
        (self.wake)();
    }

    pub fn take_actions(&self) -> Vec<Action> {
        std::mem::take(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()).actions)
    }
}
