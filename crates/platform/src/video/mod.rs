//! Video through each system's own player. The player fetches, decodes,
//! plays the sound and keeps picture and sound in step; this crate only
//! copies out the current picture as BGRA at the size asked for.
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// One picture: `width * height * 4` bytes, B, G, R, A, rows top to bottom.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// Goes up by one with every new picture.
    pub serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Loading,
    Playing,
    Paused,
    Ended,
    Failed,
}

pub trait Video {
    fn play(&self);
    fn pause(&self);
    fn seek(&self, to: Duration);
    fn set_volume(&self, volume: f32);
    fn set_muted(&self, muted: bool);
    fn state(&self) -> State;
    fn position(&self) -> Duration;
    fn duration(&self) -> Option<Duration>;
    /// The newest picture if it is newer than `after`.
    fn frame(&self, after: u64) -> Option<Frame>;
}

/// What this machine has for playing video.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Video plays.
    Ready,
    /// No video here, and nothing for the person to install: turned off,
    /// not proven on this system, or (Linux) no video helper in this build.
    Off,
    /// Linux: the helper is here, but the system's GStreamer lacks these.
    Missing(Vec<Part>),
}

/// A part of GStreamer video needs (Linux).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// GStreamer itself.
    Gstreamer,
    /// Its base plugins.
    Base,
    /// Its good plugins.
    Good,
    /// Its libav plugin (the FFmpeg decoders).
    Libav,
}

/// Whether this build can play video on this machine, and if not whether
/// something can be installed. Runs the helper on Linux (GStreamer can take
/// seconds the first time): call it off the interface thread.
///
/// Windows: always on, with no switch (the project owner's decision); a video that
/// fails shows its card and logs why. Elsewhere `IRCORD_VIDEO=0` turns video
/// off, and `IRCORD_VIDEO=1` turns it on where the backend exists but nobody
/// has watched it play yet (macOS `PROVEN` is false).
pub fn probe() -> Probe {
    #[cfg(windows)]
    {
        Probe::Ready
    }
    #[cfg(not(windows))]
    {
        let choice = std::env::var("IRCORD_VIDEO").ok();
        if choice.as_deref() == Some("0") {
            return Probe::Off;
        }
        #[allow(unused_variables)]
        let forced = choice.as_deref() == Some("1");
        #[cfg(target_os = "macos")]
        return if forced || macos::PROVEN {
            Probe::Ready
        } else {
            Probe::Off
        };
        #[cfg(target_os = "linux")]
        return linux::probe();
        #[allow(unreachable_code)]
        Probe::Off
    }
}

/// A family of Linux distributions that install GStreamer the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Debian, Ubuntu and their derivatives: apt.
    Debian,
    /// Fedora and its relatives: dnf.
    Fedora,
    /// Arch and its derivatives: pacman.
    Arch,
    /// Anything else.
    Other,
}

/// The family `os_release` (the text of `/etc/os-release`) belongs to, by its
/// `ID` and then its `ID_LIKE`.
pub fn family(os_release: &str) -> Family {
    let value = |key: &str| {
        os_release
            .lines()
            .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
            .map(|v| {
                v.trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_lowercase()
            })
            .unwrap_or_default()
    };
    let (id, like) = (value("ID"), value("ID_LIKE"));
    std::iter::once(id.as_str())
        .chain(like.split_whitespace())
        .find_map(|name| match name {
            "debian" | "ubuntu" => Some(Family::Debian),
            "fedora" | "rhel" | "centos" => Some(Family::Fedora),
            "arch" | "archlinux" => Some(Family::Arch),
            _ => None,
        })
        .unwrap_or(Family::Other)
}

/// This system's family, from `/etc/os-release` (or `/usr/lib/os-release`).
/// Read once.
pub fn this_family() -> Family {
    static FAMILY: std::sync::OnceLock<Family> = std::sync::OnceLock::new();
    *FAMILY.get_or_init(|| {
        ["/etc/os-release", "/usr/lib/os-release"]
            .iter()
            .find_map(|path| std::fs::read_to_string(path).ok())
            .map_or(Family::Other, |text| family(&text))
    })
}

/// The command that installs GStreamer with the base, good and libav
/// plugins on `family`, as the `.deb` depends on them; `None` where the
/// packages have no names known here.
pub fn install_command(family: Family) -> Option<&'static str> {
    match family {
        Family::Debian => Some(
            "sudo apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav",
        ),
        Family::Fedora => Some(
            "sudo dnf install gstreamer1-plugins-base gstreamer1-plugins-good gstreamer1-plugin-libav",
        ),
        Family::Arch => Some("sudo pacman -S --needed gst-plugins-base gst-plugins-good gst-libav"),
        Family::Other => None,
    }
}

/// Open `url` (http or https), paused, with pictures at most `width` by
/// `height` pixels.
pub fn open(url: &str, width: u32, height: u32) -> Result<Box<dyn Video>, String> {
    #[cfg(windows)]
    return windows::open(url, width, height).map(|v| Box::new(v) as Box<dyn Video>);
    #[cfg(target_os = "macos")]
    return macos::open(url, width, height).map(|v| Box::new(v) as Box<dyn Video>);
    #[cfg(target_os = "linux")]
    return linux::open(url, width, height).map(|v| Box::new(v) as Box<dyn Video>);
    #[allow(unreachable_code)]
    {
        let _ = (url, width, height);
        Err("no video backend in this build".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_distribution_finds_its_family() {
        let cases = [
            ("ID=debian\nVERSION_ID=\"12\"\n", Family::Debian),
            ("ID=ubuntu\nID_LIKE=debian\n", Family::Debian),
            ("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n", Family::Debian),
            ("ID=pop\nID_LIKE=\"ubuntu debian\"\n", Family::Debian),
            ("ID=fedora\nVERSION_ID=41\n", Family::Fedora),
            (
                "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n",
                Family::Fedora,
            ),
            (
                "ID=nobara\nID_LIKE=\"rhel centos fedora\"\n",
                Family::Fedora,
            ),
            ("ID=arch\n", Family::Arch),
            ("ID=manjaro\nID_LIKE=arch\n", Family::Arch),
            ("ID=endeavouros\nID_LIKE=arch\n", Family::Arch),
            (
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n",
                Family::Other,
            ),
            ("ID=nixos\n", Family::Other),
            ("", Family::Other),
        ];
        for (text, want) in cases {
            assert_eq!(family(text), want, "{text:?}");
        }
    }

    #[test]
    fn id_like_does_not_match_other_keys() {
        // `VERSION_ID` and `ID_LIKE` start with letters `ID` also starts
        // with; only the key itself counts.
        assert_eq!(family("VERSION_ID=arch\nID=nixos\n"), Family::Other);
    }

    #[test]
    fn each_family_installs_base_good_and_libav() {
        for (family, manager) in [
            (Family::Debian, "sudo apt install "),
            (Family::Fedora, "sudo dnf install "),
            (Family::Arch, "sudo pacman -S "),
        ] {
            let command = install_command(family).unwrap();
            assert!(command.starts_with(manager), "{command}");
            for part in ["base", "good", "libav"] {
                assert!(command.contains(part), "{command} lacks {part}");
            }
        }
        assert_eq!(install_command(Family::Other), None);
    }
}
