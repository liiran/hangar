//! The real shim, built as part of the test kit so tests can find it.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    hangar_shim::run()
}
