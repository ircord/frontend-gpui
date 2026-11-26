//! Checking for a newer IRCord and installing it.
//!
//! The release manifest is the `latest.json` that the publish workflow
//! attaches to each release. Windows, macOS and a Linux AppImage check,
//! download and install through `cargo-packager-updater`, which verifies the
//! download against the public key in `packaging/updater.pub`. Elsewhere on
//! Linux (the `.deb`, the tarball) the app only checks: a `.deb` cannot
//! replace itself without root, so the app links to [`RELEASES_URL`] instead.
//! An AppImage installs only when this copy may replace its file
//! ([`crate::appimage::updatable`]), and then starts the new one.
//!
//! Only builds with `IRCORD_UPDATES=1` (the publish workflow's builds, as
//! desktop's) update at all; [`available`] says whether this is one.

#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::OnceLock;
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
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
const PUBLIC_KEY: &str = include_str!("../../../packaging/updater.pub");

/// How long a check waits for the manifest, on every platform.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// How long an install waits: the whole download of an installer.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Whether this build checks for updates at all.
pub fn available() -> bool {
    option_env!("IRCORD_UPDATES") == Some("1")
}

/// How this copy of IRCord takes an update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// The packager's updater replaces the installed app (Windows, macOS).
    Install,
    /// The packager's updater replaces the AppImage at this path.
    #[cfg(target_os = "linux")]
    AppImage(PathBuf),
    /// Only a check: the person downloads the release themselves.
    ReleasePage,
}

/// The route for an AppImage this copy may replace (`appimage`), on Linux.
#[cfg(target_os = "linux")]
pub fn linux_route(appimage: Option<PathBuf>) -> Route {
    appimage.map_or(Route::ReleasePage, Route::AppImage)
}

/// How this copy takes an update. Worked out once.
pub fn route() -> &'static Route {
    static ROUTE: OnceLock<Route> = OnceLock::new();
    ROUTE.get_or_init(|| {
        #[cfg(target_os = "linux")]
        return linux_route(crate::appimage::updatable());
        #[cfg(any(windows, target_os = "macos"))]
        return Route::Install;
        #[allow(unreachable_code)]
        Route::ReleasePage
    })
}

/// Whether this copy installs an update itself. Where it does not (a Linux
/// `.deb`), an available update is a link to [`RELEASES_URL`].
pub fn installs() -> bool {
    *route() != Route::ReleasePage
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
    /// Whether this copy is an AppImage to replace (the other platforms have
    /// one route).
    #[cfg(target_os = "linux")]
    route: Route,
    /// What signatures are checked against: [`PUBLIC_KEY`], or a test's.
    #[cfg_attr(
        not(any(windows, target_os = "macos", target_os = "linux")),
        allow(dead_code)
    )]
    public_key: String,
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
            #[cfg(target_os = "linux")]
            route: route().clone(),
            public_key: PUBLIC_KEY.to_string(),
        })
    }

    /// The packager's update for this platform, if the manifest announces a
    /// newer one. Requests are bounded by `timeout`, so a stalled connection
    /// ends the check or install instead of leaving it running for ever.
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    fn found(&self, timeout: Duration) -> Result<Option<cargo_packager_updater::Update>> {
        let config = cargo_packager_updater::Config {
            endpoints: vec![self.endpoint.clone()],
            pubkey: self.public_key.clone(),
            windows: None,
        };
        #[allow(unused_mut)]
        let mut builder = cargo_packager_updater::UpdaterBuilder::new(self.current.clone(), config)
            .timeout(timeout);
        #[cfg(target_os = "linux")]
        if let Route::AppImage(path) = &self.route {
            builder = builder.executable_path(path);
        }
        Ok(builder.build()?.check()?)
    }

    /// A check through the packager's updater.
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    fn check_packaged(&self) -> Result<Checked> {
        let Some(found) = self.found(CHECK_TIMEOUT)? else {
            return Ok(Checked::UpToDate);
        };
        let version = Version::parse(&found.version)
            .with_context(|| format!("the release's version {:?}", found.version))?;
        Ok(Checked::Available(Update { version }))
    }

    /// Downloads, verifies and installs `update` through the packager's
    /// updater.
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    fn install_packaged(&self, update: &Update) -> Result<()> {
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
        Ok(())
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
        if let Route::AppImage(_) = self.route {
            return self.check_packaged();
        }
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

    fn install(&self, update: &Update) -> Result<()> {
        let Route::AppImage(path) = &self.route else {
            bail!("this copy does not install updates; open {RELEASES_URL}")
        };
        self.install_packaged(update)?;
        // The new AppImage is in place: a restart that fails to start is not
        // a failed install; the next start runs the new one.
        if let Err(e) = relaunch_appimage(path) {
            log::warn!("the update is installed but IRCord could not restart: {e:#}");
        }
        Ok(())
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl Updater for System {
    fn check(&self) -> Result<Checked> {
        self.check_packaged()
    }

    fn install(&self, update: &Update) -> Result<()> {
        self.install_packaged(update)?;
        #[cfg(target_os = "macos")]
        relaunch()?;
        Ok(())
    }
}

/// Starts the new AppImage at `path` once this copy has quit (the running
/// copy holds the single-instance lock), with the environment this one had
/// before its launcher changed it.
#[cfg(target_os = "linux")]
fn relaunch_appimage(path: &std::path::Path) -> Result<()> {
    let mut command = std::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec "$2""#)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(path);
    crate::appimage::system_env(&mut command);
    // The new AppImage's runtime sets these for itself.
    for var in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
        command.env_remove(var);
    }
    command.spawn().context("start the relaunch")?;
    Ok(())
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
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    /// Serves the routes `routes` makes from the server's own address (path
    /// to body) on a local port, one request per connection, until the test
    /// ends; returns that address.
    fn serve_paths(routes: impl FnOnce(&str) -> HashMap<String, Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes = routes(&base);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut request = [0u8; 4096];
                let n = stream.read(&mut request).unwrap_or(0);
                let text = String::from_utf8_lossy(&request[..n]);
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = match routes.get(&path) {
                    Some(body) => ("200 OK", body.clone()),
                    None => ("404 Not Found", Vec::new()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    /// Serves `body` as a 200 response at `/latest.json`.
    fn serve(body: &'static str) -> String {
        let base = serve_paths(|_| HashMap::from([("/latest.json".into(), body.into())]));
        format!("{base}/latest.json")
    }

    fn system(current: &str, endpoint: &str, route: Route, public_key: &str) -> System {
        System {
            current: version(current),
            endpoint: Url::parse(endpoint).unwrap(),
            route,
            public_key: public_key.to_string(),
        }
    }

    /// A throwaway key pair, in the encoding cargo-packager uses: base64 of
    /// the minisign key files.
    fn key_pair() -> (minisign::SecretKey, String) {
        let pair = minisign::KeyPair::generate_encrypted_keypair(Some(String::new())).unwrap();
        (
            pair.sk,
            STANDARD.encode(pair.pk.to_box().unwrap().to_string()),
        )
    }

    /// `data`'s signature as the publish workflow writes it.
    fn sign(key: &minisign::SecretKey, data: &[u8]) -> String {
        let signature = minisign::sign(
            None,
            key,
            std::io::Cursor::new(data),
            Some("timestamp:1\tfile:IRCord-GPUI-Linux-x64.AppImage"),
            Some("signature from cargo-packager secret key"),
        )
        .unwrap();
        STANDARD.encode(signature.to_string())
    }

    /// A release manifest offering `version` as an AppImage at `url`.
    fn appimage_manifest(version: &str, url: &str, signature: &str) -> Vec<u8> {
        serde_json::json!({
            "version": version,
            "platforms": {
                "linux-x86_64": { "url": url, "signature": signature, "format": "appimage" }
            }
        })
        .to_string()
        .into_bytes()
    }

    /// A folder under the system's temp folder, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("ircord-update-{name}-{}", std::process::id()));
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
    fn only_an_appimage_this_copy_may_replace_installs() {
        assert_eq!(linux_route(None), Route::ReleasePage);
        let path = PathBuf::from("/home/a/Applications/IRCord.AppImage");
        assert_eq!(linux_route(Some(path.clone())), Route::AppImage(path));
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
    fn a_copy_that_is_no_appimage_does_not_install() {
        let updater = system("0.1.0", MANIFEST_URL, Route::ReleasePage, PUBLIC_KEY);
        let update = Update {
            version: version("0.2.0"),
        };
        assert!(updater.install(&update).is_err());
    }

    #[test]
    fn an_appimage_checks_for_its_own_platform_entry() {
        let base = serve_paths(|_| {
            HashMap::from([(
                "/latest.json".into(),
                appimage_manifest("0.3.0", "http://127.0.0.1:1/x.AppImage", "c2ln"),
            )])
        });
        let scratch = Scratch::new("check");
        let image = scratch.0.join("IRCord.AppImage");
        std::fs::write(&image, b"old").unwrap();
        let updater = system(
            "0.1.0",
            &format!("{base}/latest.json"),
            Route::AppImage(image),
            PUBLIC_KEY,
        );
        assert_eq!(
            updater.check().unwrap(),
            Checked::Available(Update {
                version: version("0.3.0")
            })
        );
    }

    /// The AppImage at the same path is replaced by the signed download and
    /// keeps its permissions; one signed with another key is refused and the
    /// running AppImage stays.
    #[test]
    fn an_appimage_is_replaced_only_by_a_download_signed_with_the_key() {
        use std::os::unix::fs::PermissionsExt;
        let (key, public) = key_pair();
        let (stranger, _) = key_pair();
        let new_image = b"new appimage".to_vec();
        let good = sign(&key, &new_image);
        let bad = sign(&stranger, &new_image);
        // A manifest naming the download on the server's own address.
        let server = |signature: String| {
            let image = new_image.clone();
            serve_paths(move |base| {
                HashMap::from([
                    (
                        "/latest.json".to_string(),
                        appimage_manifest("0.3.0", &format!("{base}/image"), &signature),
                    ),
                    ("/image".to_string(), image),
                ])
            })
        };
        let update = Update {
            version: version("0.3.0"),
        };

        let scratch = Scratch::new("install");
        let image = scratch.0.join("IRCord.AppImage");
        std::fs::write(&image, b"old appimage").unwrap();
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o750)).unwrap();

        let refused = system(
            "0.1.0",
            &format!("{}/latest.json", server(bad)),
            Route::AppImage(image.clone()),
            &public,
        );
        assert!(refused.install_packaged(&update).is_err());
        assert_eq!(std::fs::read(&image).unwrap(), b"old appimage");

        let accepted = system(
            "0.1.0",
            &format!("{}/latest.json", server(good)),
            Route::AppImage(image.clone()),
            &public,
        );
        accepted.install_packaged(&update).unwrap();
        assert_eq!(std::fs::read(&image).unwrap(), new_image);
        let mode = std::fs::metadata(&image).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750);
    }
}
