//! What IRCord asks of the operating system besides drawing.
#[cfg(target_os = "linux")]
pub mod appimage;
pub mod audio;
pub mod desktop;
pub mod toast;
pub mod update;
pub mod video;
