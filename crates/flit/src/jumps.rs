use crossbeam_channel::{unbounded, Sender};
use flit_core::ui::{NavPrefs, NavState};
use flit_core::Location;
use flit_platform::ShellPlatform;

pub type Lists = (Vec<Location>, Vec<Location>);

pub fn lists(nav: &NavState, prefs: &NavPrefs) -> Lists {
    (nav.pinned.clone(), prefs.most_visited())
}

pub struct JumpList {
    tx: Option<Sender<Lists>>,
    shown: Option<Lists>,
}

impl JumpList {
    pub fn start<P: ShellPlatform>(on: bool) -> Self {
        let tx = on.then(|| {
            let (tx, rx) = unbounded::<Lists>();
            let run = move || {
                while let Ok(first) = rx.recv() {
                    let last = rx.try_iter().last().unwrap_or(first);
                    P::jump_list(&last.0, &last.1);
                }
            };
            if let Err(e) = std::thread::Builder::new().name("flit-jump-list".into()).spawn(run) {
                tracing::warn!("cannot start the jump list writer: {e}");
            }
            tx
        });
        Self::with(tx)
    }

    pub fn with(tx: Option<Sender<Lists>>) -> Self {
        Self { tx, shown: None }
    }

    pub fn show(&mut self, lists: Lists) {
        if self.shown.as_ref() == Some(&lists) {
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(lists.clone());
        }
        self.shown = Some(lists);
    }
}

#[cfg(test)]
#[path = "../test/jumps.rs"]
mod tests;
