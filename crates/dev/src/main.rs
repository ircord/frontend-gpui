fn main() {
    #[cfg(target_os = "macos")]
    ircord_app::run_dev();
    #[cfg(not(target_os = "macos"))]
    ircord_app::run();
}
