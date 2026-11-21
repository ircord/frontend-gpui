//! The tray icon. GPUI has none, so on Windows this uses the `tray-icon`
//! crate: closing the window keeps the app running in the tray; the tray
//! menu shows the window again or quits. Other platforms have no tray yet and
//! quit when the last window closes (see `quit_mode`).

use gpui_kit::*;

use crate::session::Session;

/// Whether closing the last window keeps the app running.
pub fn keeps_running() -> bool {
    cfg!(windows)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::time::Duration;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    struct Tray {
        _icon: TrayIcon,
    }
    impl Global for Tray {}

    /// A 32×32 icon drawn here: a blurple disc with a white "#".
    fn icon() -> Icon {
        const N: usize = 32;
        let mut rgba = vec![0u8; N * N * 4];
        for y in 0..N {
            for x in 0..N {
                let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
                let inside = dx * dx + dy * dy <= 15.5 * 15.5;
                let hash = (matches!(x, 11..=13 | 18..=20) && (7..=24).contains(&y))
                    || (matches!(y, 11..=13 | 18..=20) && (7..=24).contains(&x));
                let px = &mut rgba[(y * N + x) * 4..][..4];
                if inside {
                    px.copy_from_slice(if hash {
                        &[255, 255, 255, 255]
                    } else {
                        &[88, 101, 242, 255]
                    });
                }
            }
        }
        Icon::from_rgba(rgba, N as u32, N as u32).expect("the icon is 32×32 RGBA")
    }

    pub fn install(session: Entity<Session>, cx: &mut App) -> anyhow::Result<()> {
        let show = MenuItem::new("Show IRCord", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit])?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("IRCord")
            .with_icon(icon())
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;
        cx.set_global(Tray { _icon: icon });
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
}

#[cfg(windows)]
pub use imp::install;

#[cfg(not(windows))]
pub fn install(_session: Entity<Session>, _cx: &mut App) -> anyhow::Result<()> {
    Ok(())
}
