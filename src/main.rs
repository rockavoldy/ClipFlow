// Release builds are GUI-only: no console window flashes behind the deck.
// Debug builds keep the console so `log` output is visible during development.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    yascreenclipper_lib::run();
}
