// A release build is a windowed app with no console; it logs to
// `ircord.log` in its settings folder instead.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    ircord_app::run();
}
