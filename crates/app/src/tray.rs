//! The tray icon. GPUI has none, so this uses the `tray-icon` crate: closing
//! the window keeps the app running in the tray; the tray menu shows the
//! window again or quits.
//!
//! - Windows: the notification area.
//! - macOS: the menu bar. The icon is made on the main thread once GPUI's
//!   event loop runs, as `tray-icon` requires.
//! - Linux: a StatusNotifierItem on the session bus (the KSNI backend, no
//!   GTK). Many desktops show those (KDE, Cinnamon, XFCE, Ubuntu's GNOME with
//!   its AppIndicator extension); plain GNOME does not. When no tray host is
//!   on the bus, `install` fails, and the caller falls back to quitting when
//!   the last window closes so the app cannot be left running out of reach.

use gpui_kit::*;
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::session::Session;

/// Whether closing the last window keeps the app running. `install` failing
/// turns it off again.
pub fn keeps_running() -> bool {
    cfg!(any(windows, target_os = "macos", target_os = "linux"))
}

struct Tray {
    icon: TrayIcon,
}
impl Global for Tray {}

/// A blurple disc with a white "#", drawn at `n`×`n` (the design of
/// `packaging/icon.py`), as RGBA rows.
pub fn pixels(n: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; n * n * 4];
    let unit = 32.0 / n as f32;
    for y in 0..n {
        for x in 0..n {
            // The pixel centre on the icon's 32-unit grid.
            let (gx, gy) = ((x as f32 + 0.5) * unit, (y as f32 + 0.5) * unit);
            let inside = (gx - 16.0).powi(2) + (gy - 16.0).powi(2) <= 16.0 * 16.0;
            let bar = |at: f32| (11.0..14.0).contains(&at) || (18.0..21.0).contains(&at);
            let span = |at: f32| (7.0..25.0).contains(&at);
            let hash = (bar(gx) && span(gy)) || (bar(gy) && span(gx));
            let px = &mut rgba[(y * n + x) * 4..][..4];
            if inside {
                px.copy_from_slice(if hash {
                    &[255, 255, 255, 255]
                } else {
                    &[88, 101, 242, 255]
                });
            }
        }
    }
    rgba
}

/// The side of the tray icon in pixels on this platform.
pub fn icon_size() -> usize {
    // Linux trays scale whatever they get; a larger image stays sharp there.
    if cfg!(target_os = "linux") { 64 } else { 32 }
}

fn icon() -> Icon {
    let n = icon_size();
    Icon::from_rgba(pixels(n), n as u32, n as u32).expect("the icon is square RGBA")
}

/// Whether the tray icon is showing.
pub fn installed(cx: &App) -> bool {
    cx.has_global::<Tray>()
}

/// Replace the tray icon's picture (`icon_size()` square RGBA rows) and its
/// tooltip, as DK1's unread mark does. Nothing happens without a tray.
pub fn update(cx: &App, rgba: Vec<u8>, tooltip: &str) {
    let Some(tray) = cx.try_global::<Tray>() else {
        return;
    };
    let n = icon_size() as u32;
    match Icon::from_rgba(rgba, n, n) {
        Ok(icon) => {
            if let Err(e) = tray.icon.set_icon(Some(icon)) {
                log::warn!("tray icon: {e}");
            }
        }
        Err(e) => log::warn!("tray icon: {e}"),
    }
    if let Err(e) = tray.icon.set_tooltip(Some(tooltip)) {
        log::warn!("tray tooltip: {e}");
    }
}

/// Whether a StatusNotifier host is on the session bus to show the icon.
#[cfg(target_os = "linux")]
fn tray_host() -> anyhow::Result<()> {
    let bus = zbus::blocking::Connection::session()?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus)?;
    let name = zbus::names::BusName::try_from("org.kde.StatusNotifierWatcher")?;
    if dbus.name_has_owner(name)? {
        Ok(())
    } else {
        anyhow::bail!("no tray host on the session bus (StatusNotifierWatcher)")
    }
}

pub fn install(session: Entity<Session>, cx: &mut App) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    tray_host()?;
    let show = MenuItem::new(t!("core-tray-show"), true, None);
    let quit = MenuItem::new(t!("core-tray-quit"), true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit])?;
    let icon = TrayIconBuilder::new()
        .with_id("ircord")
        .with_title(t!("core-app-name"))
        .with_tooltip(t!("core-app-name"))
        .with_icon(icon())
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;
    cx.set_global(Tray { icon });
    let (show_id, quit_id) = (show.id().clone(), quit.id().clone());
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let mut open = false;
            let mut exit = false;
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                open |= event.id == show_id;
                exit |= event.id == quit_id;
            }
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    open = true;
                }
            }
            let session = session.clone();
            cx.update(|cx| {
                if exit {
                    cx.quit();
                } else if open {
                    crate::show_main_window(session, cx);
                }
            });
            if exit {
                break;
            }
        }
    })
    .detach();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::pixels;

    #[test]
    fn the_icon_is_a_disc_with_a_hash() {
        let n = 32;
        let at = |x: usize, y: usize| pixels(n)[(y * n + x) * 4..][..4].to_vec();
        assert_eq!(at(0, 0), [0, 0, 0, 0], "corners are transparent");
        assert_eq!(at(4, 16), [88, 101, 242, 255], "the disc is blurple");
        assert_eq!(at(12, 16), [255, 255, 255, 255], "the # is white");
    }
}
