//! Checking for a newer IRCord and installing it.
//!
//! The release manifest is the `latest.json` that the publish workflow
//! attaches to each release. Windows and macOS check, download and install
//! through `cargo-packager-updater`, which verifies the download against the
//! public key in `packaging/updater.pub`. Linux only checks: a `.deb` cannot
//! replace itself without root, so the app links to [`RELEASES_URL`] instead.
//!
//! Only builds with `IRCORD_UPDATES=1` (the publish workflow's builds, as
//! desktop's) update at all; [`available`] says whether this is one.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use semver::Version;
use url::Url;

/// The manifest of the latest release.
pub const MANIFEST_URL: &str =
    "https://github.com/ircord/frontend-gpui/releases/latest/download/latest.json";
/// The page a person downloads a release from by hand.
pub const RELEASES_URL: &str = "https://github.com/ircord/frontend-gpui/releases/latest";

/// The minisign public key the release signatures are checked against.
#[cfg(any(windows, target_os = "macos"))]
const PUBLIC_KEY: &str = include_str!("../../../packaging/updater.pub");

/// How long a check waits for the manifest, on every platform.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// How long an install waits: the whole download of an installer.
#[cfg(any(windows, target_os = "macos"))]
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Whether this build checks for updates at all.
pub fn available() -> bool {
    option_env!("IRCORD_UPDATES") == Some("1")
}

/// Whether this platform installs an update itself. Where it does not (Linux),
/// an available update is a link to [`RELEASES_URL`].
pub const fn installs() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

/// A release newer than the running copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub version: Version,
}

/// What a check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checked {
    UpToDate,
    Available(Update),
}

/// Looks for a newer release and installs one. Both block; the app calls
/// them off the interface thread.
pub trait Updater: Send + Sync {
    fn check(&self) -> Result<Checked>;
    /// Downloads, verifies and installs `update`, then restarts IRCord. On
    /// Windows the installer takes over and this process ends.
    fn install(&self, update: &Update) -> Result<()>;
}

/// The updater for this copy of IRCord, reading the release manifest at an
/// endpoint.
pub struct System {
    current: Version,
    endpoint: Url,
}

impl System {
    /// The updater for the running `current` version, reading [`MANIFEST_URL`].
    pub fn new(current: &str) -> Result<Self> {
        Self::with_endpoint(current, MANIFEST_URL)
    }

    /// As [`System::new`], reading the manifest at `endpoint`.
    pub fn with_endpoint(current: &str, endpoint: &str) -> Result<Self> {
        Ok(Self {
            current: Version::parse(current).context("the running version")?,
            endpoint: Url::parse(endpoint).context("the update endpoint")?,
        })
    }
}

/// The version a manifest announces, if it is newer than `current`.
#[cfg(target_os = "linux")]
fn newer(manifest: &str, current: &Version) -> Result<Checked> {
    let value: serde_json::Value =
        serde_json::from_str(manifest).context("the release manifest is not JSON")?;
    let announced = value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("the release manifest names no version"))?;
    let latest = Version::parse(announced)
        .with_context(|| format!("the release manifest's version {announced:?} is not one"))?;
    Ok(if latest > *current {
        Checked::Available(Update { version: latest })
    } else {
        Checked::UpToDate
    })
}

#[cfg(target_os = "linux")]
impl Updater for System {
    fn check(&self) -> Result<Checked> {
        let client = reqwest::blocking::Client::builder()
            .timeout(CHECK_TIMEOUT)
            .build()?;
        let body = client
            .get(self.endpoint.clone())
            .send()?
            .error_for_status()?
            .text()?;
        newer(&body, &self.current)
    }

    fn install(&self, _update: &Update) -> Result<()> {
        bail!("Linux does not install updates; open {RELEASES_URL}")
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl System {
    /// The packager's update for this platform, if the manifest announces a
    /// newer one.
    /// Requests are bounded by , so a stalled connection ends the
    /// check or install instead of leaving it running for ever.
    fn found(&self, timeout: Duration) -> Result<Option<cargo_packager_updater::Update>> {
        let config = cargo_packager_updater::Config {
            endpoints: vec![self.endpoint.clone()],
            pubkey: PUBLIC_KEY.to_string(),
            windows: None,
        };
        let updater = cargo_packager_updater::UpdaterBuilder::new(self.current.clone(), config)
            .timeout(timeout)
            .build()?;
        Ok(updater.check()?)
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl Updater for System {
    fn check(&self) -> Result<Checked> {
        let Some(found) = self.found(CHECK_TIMEOUT)? else {
            return Ok(Checked::UpToDate);
        };
        let version = Version::parse(&found.version)
            .with_context(|| format!("the release's version {:?}", found.version))?;
        Ok(Checked::Available(Update { version }))
    }

    fn install(&self, update: &Update) -> Result<()> {
        let Some(found) = self.found(INSTALL_TIMEOUT)? else {
            bail!("there is no newer release any more");
        };
        let version = Version::parse(&found.version)?;
        if version != update.version {
            bail!(
                "the newest release is now {version}, not {}; check again",
                update.version
            );
        }
        found.download_and_install()?;
        #[cfg(target_os = "macos")]
        relaunch()?;
        Ok(())
    }
}

/// Starts a new copy once this one has quit: opening it now would hand the
/// new start to this copy, which is still running.
#[cfg(target_os = "macos")]
fn relaunch() -> Result<()> {
    // The app is `<name>.app/Contents/MacOS/<binary>`.
    let exe = std::env::current_exe().context("the running program")?;
    let bundle = exe
        .ancestors()
        .nth(3)
        .context("the running program is not in an app bundle")?;
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open "$2""#)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(bundle)
        .spawn()
        .context("start the relaunch")?;
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    /// Serves `body` as a 200 response to the next request on a local port.
    fn serve(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{address}/latest.json")
    }

    #[test]
    fn a_newer_manifest_is_available() {
        let found = newer(r#"{"version":"0.2.0"}"#, &version("0.1.0")).unwrap();
        assert_eq!(
            found,
            Checked::Available(Update {
                version: version("0.2.0")
            })
        );
    }

    #[test]
    fn the_same_or_an_older_manifest_is_up_to_date() {
        let same = newer(r#"{"version":"0.1.0"}"#, &version("0.1.0")).unwrap();
        let older = newer(r#"{"version":"0.0.9"}"#, &version("0.1.0")).unwrap();
        assert_eq!(same, Checked::UpToDate);
        assert_eq!(older, Checked::UpToDate);
    }

    #[test]
    fn a_pre_release_is_older_than_its_release() {
        let found = newer(r#"{"version":"0.1.0-rc.1"}"#, &version("0.1.0")).unwrap();
        assert_eq!(found, Checked::UpToDate);
    }

    #[test]
    fn a_malformed_manifest_is_an_error() {
        assert!(newer("not json", &version("0.1.0")).is_err());
        assert!(newer(r#"{"notes":"no version"}"#, &version("0.1.0")).is_err());
        assert!(newer(r#"{"version":"soon"}"#, &version("0.1.0")).is_err());
    }

    #[test]
    fn check_reads_the_manifest_at_its_endpoint() {
        let endpoint = serve(r#"{"version":"0.3.0","notes":"x"}"#);
        let updater = System::with_endpoint("0.1.0", &endpoint).unwrap();
        assert_eq!(
            updater.check().unwrap(),
            Checked::Available(Update {
                version: version("0.3.0")
            })
        );
    }

    #[test]
    fn check_fails_when_nothing_answers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed = listener.local_addr().unwrap();
        drop(listener);
        let updater =
            System::with_endpoint("0.1.0", &format!("http://{closed}/latest.json")).unwrap();
        assert!(updater.check().is_err());
    }

    #[test]
    fn linux_does_not_install() {
        let updater = System::with_endpoint("0.1.0", MANIFEST_URL).unwrap();
        let update = Update {
            version: version("0.2.0"),
        };
        assert!(updater.install(&update).is_err());
        assert!(!installs());
    }
}
