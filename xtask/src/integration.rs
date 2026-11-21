//! `cargo xtask integration`: the client against a real backend.
//!
//! Builds the backend image from the sibling checkout, starts it and an Ergo
//! IRC server on a private Docker network, runs `crates/client/tests/live.rs`
//! in the build container sharing the backend's network namespace (so the
//! backend is `http://127.0.0.1:8080`, an address the client accepts), and
//! removes everything it started, pass or fail.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Ergo image the backend's own integration checks use.
const ERGO_IMAGE: &str =
    "ghcr.io/ergochat/ergo@sha256:ef885e44f7fa19101bbbc41baef11dc280dc8107465dccaf6f0860f41b48a682";
const BACKEND_IMAGE: &str = "ircord-gpui-it-backend:local";

struct Cleanup(Vec<Vec<String>>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for args in self.0.iter().rev() {
            let _ = Command::new("docker")
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

pub fn run(root: &Path) -> Result<()> {
    let backend = match std::env::var_os("IRCORD_BACKEND") {
        Some(dir) => PathBuf::from(dir),
        None => root.join("../ircord-backend"),
    };
    if !backend.join("Dockerfile").is_file() {
        bail!(
            "no backend checkout at {} (set IRCORD_BACKEND)",
            backend.display()
        );
    }
    println!("building the backend image from {}", backend.display());
    docker(&[
        "build",
        "-q",
        "-t",
        BACKEND_IMAGE,
        &backend.display().to_string(),
    ])?;

    let id = std::process::id();
    let net = format!("ircord-gpui-it-{id}");
    let ergo = format!("ircord-gpui-it-ergo-{id}");
    let server = format!("ircord-gpui-it-backend-{id}");
    let mut cleanup = Cleanup(Vec::new());

    docker(&["network", "create", &net])?;
    cleanup
        .0
        .push(vec!["network".into(), "rm".into(), net.clone()]);
    docker(&["run", "-d", "--name", &ergo, "--network", &net, ERGO_IMAGE])?;
    cleanup.0.push(vec!["rm".into(), "-f".into(), ergo.clone()]);
    docker(&[
        "run",
        "-d",
        "--name",
        &server,
        "--network",
        &net,
        "--tmpfs",
        "/data:uid=65532,gid=65532",
        "-e",
        "IRCORD_LISTEN=0.0.0.0:8080",
        "-e",
        "IRCORD_PUBLIC_URL=http://127.0.0.1:8080",
        "-e",
        "IRCORD_ALLOW_PRIVATE_IRC=true",
        "-e",
        "IRCORD_YTDLP_BINARY=disabled",
        BACKEND_IMAGE,
    ])?;
    cleanup
        .0
        .push(vec!["rm".into(), "-f".into(), server.clone()]);

    let result = crate::container::run_with(
        root,
        &[
            "--network".into(),
            format!("container:{server}"),
            "-e".into(),
            "IRCORD_IT_SERVER=http://127.0.0.1:8080".into(),
            "-e".into(),
            format!("IRCORD_IT_IRC={ergo}:6667"),
        ],
        &[
            "cargo",
            "test",
            "--package",
            "ircord-client",
            "--test",
            "live",
            "--",
            "--nocapture",
        ],
    );
    if result.is_err() {
        eprintln!("--- backend log ---");
        let _ = Command::new("docker")
            .args(["logs", "--tail", "60", &server])
            .status();
        eprintln!("--- ergo log ---");
        let _ = Command::new("docker")
            .args(["logs", "--tail", "30", &ergo])
            .status();
    }
    drop(cleanup);
    result.context("the live integration test failed")?;
    println!("integration passed");
    Ok(())
}

fn docker(args: &[&str]) -> Result<()> {
    let status = Command::new("docker")
        .args(args)
        .stdout(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        bail!("docker {} failed", args.join(" "));
    }
    Ok(())
}
