use std::marker::PhantomData;
use std::rc::Rc;

#[cfg(not(test))]
const PROCESS: libc::c_int = 1;
const IDLE: libc::c_int = 3 << 13;

pub(crate) struct IdleIo {
    previous: Option<libc::c_int>,
    thread: PhantomData<Rc<()>>,
}

impl IdleIo {
    pub(crate) fn enter() -> Self {
        let previous = current().filter(|&old| old != IDLE && old >> 13 != 1 && set(IDLE));
        Self { previous, thread: PhantomData }
    }
}

impl Drop for IdleIo {
    fn drop(&mut self) {
        if let Some(old) = self.previous {
            set(old);
        }
    }
}

#[cfg(not(test))]
fn current() -> Option<libc::c_int> {
    let value = unsafe { libc::syscall(libc::SYS_ioprio_get, PROCESS, 0) };
    (value >= 0).then_some(value as libc::c_int)
}

#[cfg(not(test))]
fn set(value: libc::c_int) -> bool {
    unsafe { libc::syscall(libc::SYS_ioprio_set, PROCESS, 0, value) == 0 }
}

#[cfg(test)]
#[path = "../test/io_priority.rs"]
mod tests;
#[cfg(test)]
use tests::{current, set};
