// No console subsystem on Windows: the shim talks over the pipes its client
// gives it, so it needs no console (and Windows starts no conhost.exe for it).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    hangar_shim::run()
}
