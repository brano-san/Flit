#![cfg(target_os = "linux")]

mod fonts;
pub mod fs;
mod gl;
mod input;
pub mod instance;
pub mod places;
pub mod shell;
mod text;
mod window;
pub mod xdg;

#[cfg(test)]
#[path = "../test/temp.rs"]
mod temp;
