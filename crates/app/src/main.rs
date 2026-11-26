// A release build is a windowed app with no console; it logs to
// `ircord.log` in its settings folder instead.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    // SAFETY: the first thing the program does; no other thread exists yet.
    #[cfg(target_os = "linux")]
    unsafe {
        ircord_platform::appimage::restore_system_env();
    }
    // One running copy: a second launch (an irc:// link clicked elsewhere,
    // the start menu again) hands its links to the first and exits.
    if ircord_app::features::desktop::hand_over() {
        return;
    }
    ircord_app::run();
}
