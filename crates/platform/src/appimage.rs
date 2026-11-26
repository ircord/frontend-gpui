//! Running as an AppImage (Linux): where the file is, whether this copy may
//! replace it with an update, its entry in the applications menu, and the
//! environment its launcher changed.
//!
//! The AppImage runtime names the file in `APPIMAGE` and mounts it at
//! `APPDIR`; the launcher inside (cargo-packager's `AppRun`) points
//! `LD_LIBRARY_PATH`, the GStreamer plugin path and a few more into that
//! mount before it starts IRCord.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The menu entry's file name, the app's own ID as the `.deb` installs it.
pub const DESKTOP_ID: &str = "com.ircord.gpui.desktop";

/// The key in the menu entry that names the AppImage it was written for.
const MARK: &str = "X-IRCord-AppImage=";

/// What the launcher points into the mounted image.
const LAUNCHER_VARS: [&str; 10] = [
    "LD_LIBRARY_PATH",
    "PATH",
    "XDG_DATA_DIRS",
    "PYTHONPATH",
    "PYTHONHOME",
    "PERLLIB",
    "QT_PLUGIN_PATH",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GSETTINGS_SCHEMA_DIR",
];

/// The AppImage this copy runs from, when it runs from one.
pub fn path() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("APPIMAGE")?);
    path.is_file().then_some(path)
}

/// The AppImage this copy runs from, when this copy may replace it with an
/// update.
pub fn updatable() -> Option<PathBuf> {
    path().filter(|p| replaceable(p))
}

/// Whether the updater can swap `file` for a newer one: it renames `file`
/// aside and writes the new one in its place, so `file` must not be read-only
/// and its folder must take new files.
pub fn replaceable(file: &Path) -> bool {
    let Ok(meta) = file.metadata() else {
        return false;
    };
    if !meta.is_file() || meta.permissions().readonly() {
        return false;
    }
    let Some(dir) = file.parent() else {
        return false;
    };
    let probe = dir.join(format!(".ircord-update-check-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// `value` (a `:` list) without its entries inside `appdir`; `None` when
/// nothing is left.
fn outside(value: &str, appdir: &str) -> Option<String> {
    let appdir = appdir.trim_end_matches('/');
    let kept: Vec<&str> = value
        .split(':')
        .filter(|entry| {
            !entry.is_empty()
                && *entry != appdir
                && !entry
                    .strip_prefix(appdir)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join(":"))
}

/// Gives this process the system's environment back, so every program it
/// starts (the browser through xdg-open, zenity, the video helper) runs on
/// the system's libraries rather than in the image. The app itself is already
/// loaded: its own libraries come from its runpath, not `LD_LIBRARY_PATH`.
/// Outside an AppImage it changes nothing.
///
/// # Safety
///
/// Changes the process environment: call it first thing in `main`, before
/// any other thread exists.
pub unsafe fn restore_system_env() {
    let Some(appdir) = std::env::var("APPDIR").ok().filter(|d| !d.is_empty()) else {
        return;
    };
    for var in LAUNCHER_VARS {
        let Ok(value) = std::env::var(var) else {
            continue;
        };
        // SAFETY: the caller promises no other thread runs yet.
        unsafe {
            match outside(&value, &appdir) {
                Some(kept) => std::env::set_var(var, kept),
                None => std::env::remove_var(var),
            }
        }
    }
}

/// Gives `command` the system's environment back, as it was before the
/// AppImage launcher pointed it into the image: for a program that uses the
/// system's libraries (the video helper and its GStreamer) or another
/// AppImage. Outside an AppImage it changes nothing.
pub fn system_env(command: &mut Command) {
    let Some(appdir) = std::env::var("APPDIR").ok().filter(|d| !d.is_empty()) else {
        return;
    };
    for var in LAUNCHER_VARS {
        let Ok(value) = std::env::var(var) else {
            continue;
        };
        match outside(&value, &appdir) {
            Some(kept) => command.env(var, kept),
            None => command.env_remove(var),
        };
    }
}

/// Where the menu entry is for this AppImage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    /// There is none.
    Missing,
    /// It starts this AppImage.
    Current,
    /// It was written for an AppImage at another path that is gone (this
    /// one, moved).
    Moved,
    /// Another entry with the app's ID: for another copy of the AppImage
    /// still there, `install.sh`'s, or one made by hand.
    Other,
}

/// `$XDG_DATA_HOME`, by default `~/.local/share`.
pub fn data_home() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(".local/share"))
        .ok_or_else(|| io::Error::other("no home folder"))
}

/// Where the menu entry for `appimage` stands under `data` (a data home).
pub fn menu_entry(data: &Path, appimage: &Path) -> MenuEntry {
    let Ok(text) = std::fs::read_to_string(data.join("applications").join(DESKTOP_ID)) else {
        return MenuEntry::Missing;
    };
    match text.lines().find_map(|line| line.strip_prefix(MARK)) {
        Some(path) if Path::new(path) == appimage => MenuEntry::Current,
        // The AppImage it was written for is gone: this one is it, moved.
        Some(path) if !Path::new(path).exists() => MenuEntry::Moved,
        // Another copy that is still there, or an entry made otherwise.
        _ => MenuEntry::Other,
    }
}

/// Writes the menu entry and icons for `appimage` under `data`, for the
/// current user, replacing any entry with the app's ID there.
pub fn add_menu_entry(data: &Path, appimage: &Path) -> io::Result<()> {
    let apps = data.join("applications");
    let icons = data.join("icons/hicolor");
    let svg = icons.join("scalable/apps/com.ircord.gpui.svg");
    let png = icons.join("256x256/apps/com.ircord.gpui.png");
    for dir in [Some(apps.as_path()), svg.parent(), png.parent()]
        .into_iter()
        .flatten()
    {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&svg, include_bytes!("../../../packaging/linux/ircord.svg"))?;
    std::fs::write(&png, include_bytes!("../../../packaging/linux/ircord.png"))?;
    let entry = desktop_entry(
        include_str!("../../../packaging/linux/com.ircord.gpui.desktop"),
        appimage,
    );
    // Written aside and renamed, so a menu never reads half an entry.
    let target = apps.join(DESKTOP_ID);
    let partial = apps.join(format!(".{DESKTOP_ID}.new"));
    std::fs::write(&partial, entry)?;
    std::fs::rename(&partial, &target)?;
    // Menus that cache entries pick it up sooner; neither tool is required.
    let _ = Command::new("update-desktop-database")
        .arg(&apps)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = Command::new("gtk-update-icon-cache")
        .arg("-q")
        .arg(&icons)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(())
}

/// The packaged menu entry (`template`), starting `appimage` and naming it in
/// [`MARK`].
pub fn desktop_entry(template: &str, appimage: &Path) -> String {
    let path = appimage.to_string_lossy();
    let mut lines: Vec<String> = template
        .lines()
        .map(|line| match line.strip_prefix("Exec=") {
            Some(_) => format!("Exec={} %u", exec_argument(&path)),
            None => line.to_string(),
        })
        .collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with('[') && l != "[Desktop Entry]")
        .unwrap_or(lines.len());
    lines.insert(at, format!("{MARK}{path}"));
    lines.join("\n") + "\n"
}

/// `path` as one argument of an `Exec` key, quoted and escaped as the Desktop
/// Entry Specification asks: quoted when it holds a reserved character, with
/// `"`, `` ` ``, `$` and `\` escaped inside the quotes, then `\` doubled as in
/// any string value, and `%` doubled so it is not a field code.
fn exec_argument(path: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(',
        ')', '`',
    ];
    let argument = if path.contains(RESERVED) {
        let mut quoted = String::from("\"");
        for c in path.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        quoted
    } else {
        path.to_string()
    };
    argument.replace('\\', "\\\\").replace('%', "%%")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder under the system's temp folder, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("ircord-appimage-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn launcher_entries_go_and_the_systems_stay() {
        let appdir = "/tmp/.mount_IRCordAb12";
        assert_eq!(
            outside(
                "/tmp/.mount_IRCordAb12/usr/lib/:/tmp/.mount_IRCordAb12/lib/:/usr/local/lib",
                appdir
            )
            .as_deref(),
            Some("/usr/local/lib")
        );
        // AppRun leaves a trailing `:` where the variable was unset.
        assert_eq!(
            outside("/tmp/.mount_IRCordAb12/usr/lib/gstreamer-1.0:", appdir),
            None
        );
        assert_eq!(outside("/tmp/.mount_IRCordAb12/usr/", appdir), None);
        // A folder that only starts with the same letters is not inside it.
        assert_eq!(
            outside("/tmp/.mount_IRCordAb12x/lib:/usr/bin", appdir).as_deref(),
            Some("/tmp/.mount_IRCordAb12x/lib:/usr/bin")
        );
    }

    #[test]
    fn the_entry_starts_the_appimage_and_names_it() {
        let template = "[Desktop Entry]\nType=Application\nExec=ircord %u\nIcon=com.ircord.gpui\n";
        let entry = desktop_entry(template, Path::new("/home/a/Applications/IRCord.AppImage"));
        assert_eq!(
            entry,
            "[Desktop Entry]\nType=Application\nExec=/home/a/Applications/IRCord.AppImage %u\n\
             Icon=com.ircord.gpui\nX-IRCord-AppImage=/home/a/Applications/IRCord.AppImage\n"
        );
    }

    #[test]
    fn the_entry_is_written_into_the_main_group() {
        let template = "[Desktop Entry]\nExec=ircord %u\n[Desktop Action new]\nExec=ircord --new\n";
        let entry = desktop_entry(template, Path::new("/a/I.AppImage"));
        let mark = entry.find(MARK).unwrap();
        assert!(
            mark < entry.find("[Desktop Action new]").unwrap(),
            "{entry}"
        );
    }

    #[test]
    fn a_path_with_reserved_characters_is_quoted_and_escaped() {
        assert_eq!(
            exec_argument("/home/a/My Apps/I.AppImage"),
            "\"/home/a/My Apps/I.AppImage\""
        );
        // In the file: "\\$HOME\\\\x\\"" — the argument "$HOME\x" after both
        // unescapings.
        assert_eq!(
            exec_argument("/a/$b\\c\"d"),
            "\"/a/\\\\$b\\\\\\\\c\\\\\"d\""
        );
        assert_eq!(exec_argument("/a/100%.AppImage"), "/a/100%%.AppImage");
    }

    #[test]
    fn the_menu_entry_is_found_for_its_own_appimage() {
        let scratch = Scratch::new("menu");
        let data = scratch.0.join("share");
        let here = scratch.0.join("Applications/IRCord.AppImage");
        assert_eq!(menu_entry(&data, &here), MenuEntry::Missing);

        add_menu_entry(&data, &here).unwrap();
        assert_eq!(menu_entry(&data, &here), MenuEntry::Current);
        assert!(
            data.join("icons/hicolor/256x256/apps/com.ircord.gpui.png")
                .is_file()
        );
        assert!(
            data.join("icons/hicolor/scalable/apps/com.ircord.gpui.svg")
                .is_file()
        );
        // Run from elsewhere: the first copy is gone, so this is it, moved;
        // while the first copy is there, it is another copy.
        let moved = scratch.0.join("Elsewhere/IRCord.AppImage");
        assert_eq!(menu_entry(&data, &moved), MenuEntry::Moved);
        std::fs::create_dir_all(here.parent().unwrap()).unwrap();
        std::fs::write(&here, b"image").unwrap();
        assert_eq!(menu_entry(&data, &moved), MenuEntry::Other);

        std::fs::write(
            data.join("applications").join(DESKTOP_ID),
            "[Desktop Entry]\nExec=/home/a/.local/bin/ircord %u\n",
        )
        .unwrap();
        assert_eq!(menu_entry(&data, &here), MenuEntry::Other);
    }

    #[test]
    fn only_a_writable_file_in_a_writable_folder_is_replaceable() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("replace");
        let file = scratch.0.join("IRCord.AppImage");
        assert!(!replaceable(&file), "a missing file");
        std::fs::write(&file, b"image").unwrap();
        assert!(replaceable(&file));
        assert_eq!(
            std::fs::read_dir(&scratch.0).unwrap().count(),
            1,
            "the probe is gone"
        );

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(!replaceable(&file), "a read-only file");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();

        std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(0o555)).unwrap();
        let in_locked = replaceable(&file);
        std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root writes into any folder; the test container runs as a user.
        let root = std::fs::metadata("/root").is_ok_and(|_| std::fs::read_dir("/root").is_ok());
        assert!(root || !in_locked, "a folder that takes no new files");
    }
}
