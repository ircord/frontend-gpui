// A release build is a windowed app with no console; it logs to
// `ircord.log` in its settings folder instead.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    // One running copy: a second launch (an irc:// link clicked elsewhere,
    // the start menu again) hands its links to the first and exits.
    if ircord_app::features::desktop::hand_over() {
        return;
    }
    ircord_app::run();
}
