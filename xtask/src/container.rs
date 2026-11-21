//! The build container: an image from `tools/build.Dockerfile`, tagged by the
//! file's hash, with the host's cargo registry and the cargo-xwin download
//! cache mounted so repeated builds reuse them.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";

pub fn image(root: &Path) -> Result<String> {
    let dockerfile = root.join("tools/build.Dockerfile");
    let text = std::fs::read(&dockerfile).context("read tools/build.Dockerfile")?;
    let tag = format!("ircord-gpui-build:{:016x}", fnv1a(&text));
    let present = Command::new("docker")
        .args(["image", "inspect", &tag])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("run docker")?
        .success();
    if !present {
        println!("building {tag} from tools/build.Dockerfile");
        let status = Command::new("docker")
            .args(["build", "-t", &tag, "-f"])
            .arg(&dockerfile)
            .arg(root.join("tools"))
            .status()?;
        if !status.success() {
            bail!("docker build of the build image failed");
        }
    }
    Ok(tag)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is set"))
}

/// `docker run` arguments shared by every container step.
pub fn base_args(root: &Path) -> Result<Vec<String>> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cargo"));
    let xwin_cache = home().join(".cache/cargo-xwin");
    for dir in [
        cargo_home.join("registry"),
        cargo_home.join("git"),
        xwin_cache.clone(),
    ] {
        std::fs::create_dir_all(&dir)?;
    }
    let uid = output("id", &["-u"])?;
    let gid = output("id", &["-g"])?;
    let mut args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "--init".into(),
        "--user".into(),
        format!("{uid}:{gid}"),
        "-e".into(),
        "HOME=/tmp".into(),
        "-e".into(),
        "CARGO_HOME=/cargo-home".into(),
        "-e".into(),
        "CARGO_TERM_COLOR=always".into(),
        "-e".into(),
        "XWIN_CACHE_DIR=/xwin-cache".into(),
        "-e".into(),
        "XWIN_ACCEPT_LICENSE=1".into(),
        "-v".into(),
        format!("{}:/work", root.display()),
        "-v".into(),
        format!(
            "{}:/cargo-home/registry",
            cargo_home.join("registry").display()
        ),
        "-v".into(),
        format!("{}:/cargo-home/git", cargo_home.join("git").display()),
        "-v".into(),
        format!("{}:/xwin-cache", xwin_cache.display()),
        "-w".into(),
        "/work".into(),
    ];
    if let Ok(filter) = std::env::var("RUST_LOG") {
        args.push("-e".into());
        args.push(format!("RUST_LOG={filter}"));
    }
    Ok(args)
}

/// Run `cmd` in the build container with the repository at `/work`.
pub fn run(root: &Path, cmd: &[&str]) -> Result<()> {
    run_with(root, &[], cmd)
}

pub fn run_with(root: &Path, extra: &[String], cmd: &[&str]) -> Result<()> {
    let image = image(root)?;
    let mut args = base_args(root)?;
    args.extend(extra.iter().cloned());
    args.push(image);
    args.extend(cmd.iter().map(|s| s.to_string()));
    let status = Command::new("docker").args(&args).status()?;
    if !status.success() {
        // Show the command up to an inline script, not the script itself.
        let end = cmd
            .iter()
            .position(|a| *a == "-c")
            .map_or(cmd.len(), |i| i + 1);
        let shown = cmd[..end].join(" ");
        bail!("`{shown}` failed in the build container");
    }
    Ok(())
}

pub fn build_windows(root: &Path) -> Result<()> {
    run(
        root,
        &[
            "cargo",
            "xwin",
            "build",
            "--release",
            "--target",
            WINDOWS_TARGET,
            "--package",
            "ircord-app",
            "--bin",
            "ircord",
        ],
    )?;
    let exe = root.join(format!("target/{WINDOWS_TARGET}/release/ircord.exe"));
    copy_to_dist(root, &exe, "ircord.exe")
}

pub fn build_linux(root: &Path) -> Result<()> {
    run(
        root,
        &[
            "cargo",
            "build",
            "--release",
            "--package",
            "ircord-app",
            "--bin",
            "ircord",
            "--package",
            "ircord-video-gst",
            "--bin",
            "ircord-video",
        ],
    )?;
    copy_to_dist(root, &root.join("target/release/ircord"), "ircord")?;
    copy_to_dist(
        root,
        &root.join("target/release/ircord-video"),
        "ircord-video",
    )?;
    package_linux(root)
}

/// `dist/ircord-linux-x86_64.tar.gz`: the binary, the `ircord-video` helper,
/// a desktop entry, the icon and `install.sh`, which installs them for the
/// current user.
fn package_linux(root: &Path) -> Result<()> {
    let stage = root.join("target/package/ircord");
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    std::fs::create_dir_all(&stage)?;
    let linux = root.join("packaging/linux");
    for (from, to) in [
        (root.join("dist/ircord"), "ircord"),
        (root.join("dist/ircord-video"), "ircord-video"),
        (
            linux.join("com.ircord.gpui.desktop"),
            "com.ircord.gpui.desktop",
        ),
        (linux.join("ircord.svg"), "ircord.svg"),
        (linux.join("ircord.png"), "ircord.png"),
        (linux.join("install.sh"), "install.sh"),
    ] {
        std::fs::copy(&from, stage.join(to)).with_context(|| format!("copy {}", from.display()))?;
    }
    let tarball = root.join("dist/ircord-linux-x86_64.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&tarball)
        .arg("-C")
        .arg(root.join("target/package"))
        .args(["--owner=0", "--group=0", "ircord"])
        .status()
        .context("run tar")?;
    if !status.success() {
        bail!("tar failed");
    }
    let size = std::fs::metadata(&tarball)?.len();
    println!("built {} ({:.1} MB)", tarball.display(), size as f64 / 1e6);
    Ok(())
}

pub fn copy_to_dist(root: &Path, built: &Path, name: &str) -> Result<()> {
    let dist = root.join("dist");
    std::fs::create_dir_all(&dist)?;
    let to = dist.join(name);
    std::fs::copy(built, &to).with_context(|| format!("copy {}", built.display()))?;
    let size = std::fs::metadata(&to)?.len();
    println!("built {} ({:.1} MB)", to.display(), size as f64 / 1e6);
    Ok(())
}

fn output(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd).args(args).output()?;
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
