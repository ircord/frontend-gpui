//! Notifications with what GPUI's own leave out: the app icon, a picture
//! (the sender's avatar) and a sound. A click comes back as the tag.
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub struct Toast {
    /// Identifies the notification; a later one with the same tag replaces it.
    pub tag: String,
    pub title: String,
    pub body: String,
    /// A picture file on disk (PNG or JPEG): the sender's avatar.
    pub picture: Option<PathBuf>,
    /// Let the system play its notification sound.
    pub sound: bool,
}

/// The app as the system should name it.
pub struct Identity {
    /// `com.ircord.gpui`
    pub id: String,
    pub name: String,
    /// The app icon as a PNG on disk, where the platform wants a file.
    pub icon: Option<PathBuf>,
}

/// Called with a toast's tag when the person activates it, on any thread.
pub type Clicked = Arc<dyn Fn(String) + Send + Sync>;

/// Show `toast`. Call on the main thread.
pub fn show(identity: &Identity, toast: Toast, clicked: Clicked) -> Result<(), String> {
    #[cfg(windows)]
    return windows::show(identity, toast, clicked);
    #[cfg(target_os = "linux")]
    return linux::show(identity, toast, clicked);
    #[cfg(target_os = "macos")]
    return macos::show(identity, toast, clicked);
    #[allow(unreachable_code)]
    {
        let _ = (identity, toast, clicked);
        Err("no notifications on this system".into())
    }
}
