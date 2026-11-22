#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| flit_fuzz::wsl_agent(bytes));
