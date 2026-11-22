#[derive(Debug, PartialEq, Eq)]
pub enum Idle {
    Exit,
    Wait,
    Open,
}

pub fn idle(requested: bool, tray: Option<bool>, quitting: bool, pending_open: bool) -> Idle {
    if quitting {
        Idle::Exit
    } else if pending_open {
        Idle::Open
    } else if requested && tray != Some(false) {
        Idle::Wait
    } else {
        Idle::Exit
    }
}

#[cfg(test)]
#[path = "../../test/ui/background.rs"]
mod tests;
