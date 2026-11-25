use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

const DX_VERSION: &str = "0.7.10";
const LOCAL_SIGNING_NAME: &str = "IRCord Local Development";

pub fn run(root: &Path, hot_patch: bool) -> Result<()> {
    let dx = dx_path();
    let version = Command::new(&dx)
        .arg("--version")
        .output()
        .with_context(|| {
            format!(
                "run {}; install Dioxus CLI {DX_VERSION} or set IRCORD_DX",
                dx.display()
            )
        })?;
    let version = String::from_utf8_lossy(&version.stdout);
    if version
        .split_whitespace()
        .take(2)
        .ne(["dioxus", DX_VERSION])
    {
        bail!(
            "{} must be Dioxus CLI {DX_VERSION}; set IRCORD_DX to its executable",
            dx.display()
        );
    }

    let mut command = Command::new(dx);
    command.current_dir(root).args([
        "serve",
        "--platform",
        env::consts::OS,
        "--package",
        "ircord-dev",
        "--bin",
        "ircord-dev",
        "--features",
        "hot-reload",
        "--addr",
        "127.0.0.1",
    ]);
    if env::consts::OS == "macos"
        && let Some(identity) = macos_signing_identity()?
    {
        move_legacy_reload_page(root)?;
        // Keep the trusted app at a fixed path. Dioxus 0.7.10 writes
        // a web index into the app bundle when it applies a hot patch.
        command
            .args([
                "--renderer",
                "native",
                "--codesign",
                "true",
                "--apple-team-id",
            ])
            .arg(identity)
            .arg("--apple-entitlements")
            .arg(root.join("crates/dev/Entitlements.plist"));
    } else if hot_patch {
        command.arg("--hot-patch");
    }
    if env::var_os("RUST_LOG").is_none() {
        command.env("RUST_LOG", "warn,gpui::hot_reload=info");
    }
    if !command
        .status()
        .context("run the Dioxus development server")?
        .success()
    {
        bail!("Dioxus development server exited unsuccessfully");
    }
    Ok(())
}

fn move_legacy_reload_page(root: &Path) -> Result<()> {
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(|dir| root.join(dir))
        .unwrap_or_else(|| root.join("target"));
    let output = target.join("dx/ircord-dev/debug/macos");
    let page = output.join("IrcordDev.app/index.html");
    let html = match fs::read_to_string(&page) {
        Ok(html) => html,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("read the old Dioxus reload page"),
    };
    if html.contains("__dx-toast-msg") && html.contains("non-hot-reloadable change") {
        fs::rename(&page, output.join("ircord-dev-legacy-index.html"))
            .context("move the old Dioxus reload page out of the app bundle")?;
    }
    Ok(())
}

fn macos_signing_identity() -> Result<Option<String>> {
    if let Ok(identity) = env::var("IRCORD_DEV_SIGNING_IDENTITY") {
        let identity = identity.trim();
        if identity.is_empty() || identity == "-" {
            bail!("IRCORD_DEV_SIGNING_IDENTITY must name a code-signing certificate");
        }
        return Ok(Some(identity.to_owned()));
    }
    let identities = Command::new("/usr/bin/security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()
        .context("find local macOS code-signing identities")?;
    if !identities.status.success() {
        return Ok(None);
    }
    let output = String::from_utf8_lossy(&identities.stdout);
    Ok(output.lines().find_map(|line| {
        let (prefix, name) = line.split_once('"')?;
        let (name, _) = name.split_once('"')?;
        if name != LOCAL_SIGNING_NAME {
            return None;
        }
        prefix.split_whitespace().nth(1).map(str::to_owned)
    }))
}

fn dx_path() -> PathBuf {
    if let Some(path) = env::var_os("IRCORD_DX") {
        return path.into();
    }
    let cargo_home = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::home_dir().map(|home| home.join(".cargo")));
    if let Some(path) = cargo_home.map(|home| {
        home.join("bin")
            .join(format!("dx{}", env::consts::EXE_SUFFIX))
    }) && path.is_file()
    {
        return path;
    }
    "dx".into()
}
