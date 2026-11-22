//! `cargo xtask build macos`: on a Mac, build `dist/IRCord GPUI.app` for the
//! Mac's own architecture. GPUI's macOS backend needs Apple's SDK and Metal
//! compiler, which only exist on a Mac, so this runs natively, not in the
//! build container. The bundle is signed ad hoc (no Apple developer ID), so
//! Gatekeeper asks once; see README.md. `cargo xtask package macos` packs it.

use crate::package::PRODUCT_NAME;
use crate::run;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Builds and signs the bundle and returns its path.
pub fn build(root: &Path) -> Result<PathBuf> {
    if !cfg!(target_os = "macos") {
        bail!(
            "`cargo xtask build macos` runs on a Mac (with Xcode); \
             it cannot cross-build from here. See README.md."
        );
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    run(Command::new(cargo).current_dir(root).args([
        "build",
        "--release",
        "--locked",
        "--package",
        "ircord-app",
        "--bin",
        "ircord",
    ]))?;

    let app = root.join(format!("dist/{PRODUCT_NAME}.app"));
    if app.exists() {
        std::fs::remove_dir_all(&app)?;
    }
    let contents = app.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS"))?;
    std::fs::create_dir_all(contents.join("Resources"))?;
    std::fs::copy(
        root.join("target/release/ircord"),
        contents.join("MacOS/ircord"),
    )
    .context("copy the binary")?;
    std::fs::copy(
        root.join("packaging/macos/ircord.icns"),
        contents.join("Resources/ircord.icns"),
    )
    .context("copy the icon")?;
    let plist = std::fs::read_to_string(root.join("packaging/macos/Info.plist"))?
        .replace("@VERSION@", env!("CARGO_PKG_VERSION"));
    std::fs::write(contents.join("Info.plist"), plist)?;
    run(Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app))?;
    println!("built {}", app.display());
    Ok(app)
}
