// Hide the console window in release builds; keep it in debug so panics and
// `println!` from the Rust side stay visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    dsh_phl_lib::run()
}
