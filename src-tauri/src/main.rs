// Release builds on Windows must not allocate a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kadr_lib::run()
}
