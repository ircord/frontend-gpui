use anyhow::{Context, Result, bail};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const DX_VERSION: &str = "0.7.10";

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
    if hot_patch {
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
