//! Linux: the XDG notification service through `notify-rust`, as GPUI's own
//! backend does, plus the icon and the picture.

use notify_rust::{Hint, Notification};

use super::{Clicked, Identity, Toast};

pub fn show(identity: &Identity, toast: Toast, clicked: Clicked) -> Result<(), String> {
    let mut n = Notification::new();
    n.appname(&identity.name)
        .summary(&toast.title)
        .body(&toast.body)
        // The installed icon's name (packaging/linux), also the desktop entry's.
        .icon(&identity.id)
        .hint(Hint::DesktopEntry(identity.id.clone()))
        .action("default", "default");
    if let Some(picture) = toast.picture.as_ref().and_then(|p| p.to_str()) {
        n.hint(Hint::ImagePath(picture.to_string()));
    }
    // The app plays its own chime on Linux: desktops differ on toast sounds.
    n.hint(Hint::SuppressSound(true));
    let built = n.finalize();
    let tag = toast.tag;
    std::thread::Builder::new()
        .name("toast".into())
        .spawn(move || match built.show() {
            Ok(handle) => handle.wait_for_action(|action| {
                if action == "default" {
                    clicked(tag);
                }
            }),
            Err(e) => log::warn!("could not show the notification: {e}"),
        })
        .map(drop)
        .map_err(|e| e.to_string())
}
