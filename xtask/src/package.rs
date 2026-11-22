//! `cargo xtask package linux|windows|macos`: the installers a release ships,
//! left in `dist/` under their release names.
//!
//! Linux and Windows build in the build container and are packaged there by
//! cargo-packager from `packaging/Packager.toml`: a `.deb`, and an NSIS setup
//! made from the cross-built `.exe`. macOS builds on a Mac; `macos.rs`
//! assembles and signs the bundle, and this packs it as a disk image and as the
//! `.app.tar.gz` the updater installs.

use crate::{container, macos};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// What people see: the Start menu, the install folder, the Mac bundle. It is
/// not "IRCord" so it installs beside the IRCord desktop app.
pub const PRODUCT_NAME: &str = "IRCord GPUI";

pub const LINUX_DEB: &str = "IRCord-GPUI-Linux-x64.deb";
pub const WINDOWS_SETUP: &str = "IRCord-GPUI-Windows-x64-setup.exe";
pub const MACOS_DMG: &str = "IRCord-GPUI-macOS-Apple-Silicon.dmg";
pub const MACOS_UPDATE: &str = "IRCord-GPUI-macOS-Apple-Silicon.app.tar.gz";

/// Where the repository is mounted in the build container.
const CONTAINER_ROOT: &str = "/work";

pub fn linux(root: &Path) -> Result<()> {
    container::build_linux(root)?;
    let out = packager(root, &["--formats", "deb"], "linux")?;
    collect(root, &out, ".deb", LINUX_DEB)
}

pub fn windows(root: &Path) -> Result<()> {
    container::build_windows(root)?;
    let out = packager(
        root,
        &["--formats", "nsis", "--target", container::WINDOWS_TARGET],
        "windows",
    )?;
    collect(root, &out, "-setup.exe", WINDOWS_SETUP)
}

pub fn macos(root: &Path) -> Result<()> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        bail!("`cargo xtask package macos` runs on an Apple Silicon Mac (with Xcode)");
    }
    let app = macos::build(root)?;
    let dist = root.join("dist");
    let bundle = app.file_name().context("the bundle has a name")?.to_owned();

    // The updater unpacks this over the installed bundle, dropping the
    // archive's first path component, so the bundle is the archive's only
    // top-level entry. COPYFILE_DISABLE keeps macOS's `._` files out.
    let update = dist.join(MACOS_UPDATE);
    run(Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .arg("-czf")
        .arg(&update)
        .arg("-C")
        .arg(&dist)
        .arg(&bundle))?;
    report(&update)?;

    // The disk image holds the bundle and a link to /Applications to drag it to.
    let stage = root.join("target/package/dmg");
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    std::fs::create_dir_all(&stage)?;
    run(Command::new("ditto").arg(&app).arg(stage.join(&bundle)))?;
    run(Command::new("ln")
        .args(["-s", "/Applications"])
        .arg(stage.join("Applications")))?;
    let dmg = dist.join(MACOS_DMG);
    // hdiutil now and then fails with "Resource busy" on a busy machine (often
    // on hosted CI runners); a second attempt a moment later goes through.
    for attempt in 1..=3 {
        if dmg.exists() {
            std::fs::remove_file(&dmg)?;
        }
        let made = run(Command::new("hdiutil")
            .args([
                "create",
                "-volname",
                PRODUCT_NAME,
                "-fs",
                "HFS+",
                "-format",
                "UDZO",
            ])
            .arg("-srcfolder")
            .arg(&stage)
            .arg(&dmg));
        match made {
            Ok(()) => break,
            Err(e) if attempt < 3 => {
                eprintln!("{e}; trying again");
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
            Err(e) => return Err(e),
        }
    }
    report(&dmg)
}

/// Runs cargo-packager in the build container on the rendered config, with
/// its output in `target/packager/<platform>`, which is returned.
fn packager(root: &Path, args: &[&str], platform: &str) -> Result<PathBuf> {
    let config = root.join("target/packager/Packager.toml");
    std::fs::create_dir_all(config.parent().expect("has a parent"))?;
    let template = std::fs::read_to_string(root.join("packaging/Packager.toml"))
        .context("read packaging/Packager.toml")?;
    std::fs::write(
        &config,
        render(&template, env!("CARGO_PKG_VERSION"), CONTAINER_ROOT),
    )?;

    let out = root.join("target/packager").join(platform);
    if out.exists() {
        std::fs::remove_dir_all(&out)?;
    }
    let config_arg = format!("{CONTAINER_ROOT}/target/packager/Packager.toml");
    let out_arg = format!("{CONTAINER_ROOT}/target/packager/{platform}");
    let mut cmd = vec![
        "cargo",
        "packager",
        "--config",
        &config_arg,
        "--out-dir",
        &out_arg,
    ];
    cmd.extend_from_slice(args);
    container::run(root, &cmd)?;
    Ok(out)
}

fn render(template: &str, version: &str, root: &str) -> String {
    template
        .replace("@VERSION@", version)
        .replace("@ROOT@", root)
}

/// Copies the one file in `out` whose name ends with `suffix` to
/// `dist/<name>`.
fn collect(root: &Path, out: &Path, suffix: &str, name: &str) -> Result<()> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(out).with_context(|| format!("read {}", out.display()))? {
        let path = entry?.path();
        if path.is_file()
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(suffix))
        {
            found.push(path);
        }
    }
    let [built] = found.as_slice() else {
        bail!(
            "expected one *{suffix} in {}, found {}",
            out.display(),
            found.len()
        );
    };
    container::copy_to_dist(root, built, name)
}

fn report(path: &Path) -> Result<()> {
    let size = std::fs::metadata(path)?.len();
    println!("built {} ({:.1} MB)", path.display(), size as f64 / 1e6);
    Ok(())
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("run {:?}", cmd.get_program()))?;
    if !status.success() {
        bail!("{:?} failed", cmd.get_program());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_renders_with_no_placeholder_left() {
        let template = include_str!("../../packaging/Packager.toml");
        let config = render(template, "1.2.3", "/work");
        assert!(!config.contains('@'), "{config}");
        assert!(config.contains("version = \"1.2.3\""));
        assert!(config.contains("binariesDir = \"/work/dist\""));
        assert!(config.contains(&format!("productName = \"{PRODUCT_NAME}\"")));
    }

    #[test]
    fn asset_names_carry_the_product_name() {
        for name in [LINUX_DEB, WINDOWS_SETUP, MACOS_DMG, MACOS_UPDATE] {
            assert!(name.starts_with("IRCord-GPUI-"), "{name}");
        }
    }
}
