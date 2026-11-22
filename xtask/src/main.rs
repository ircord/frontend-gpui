//! Repository tasks: `cargo xtask <command>`.
//!
//! GPUI needs a C toolchain and Linux system libraries, and the Windows build
//! needs clang, lld and the Microsoft libraries `cargo-xwin` fetches. All of it
//! runs inside the image built from `tools/build.Dockerfile`, so the only
//! things a host needs are Rust, git and Docker. The macOS build is the
//! exception: it runs on a Mac, without Docker (see `macos.rs`).

mod capture;
mod container;
mod contract;
mod integration;
mod macos;
mod package;
mod release;

use anyhow::{Result, bail};
use std::path::PathBuf;

const USAGE: &str = "\
usage: cargo xtask <command>

  contract [--check]   refresh (or check) crates/contract from the backend checkout
  build windows        cross-build dist/ircord.exe in the build container
  build linux          build dist/ircord and dist/ircord-linux-x86_64.tar.gz in the build container
  build macos          build dist/IRCord GPUI.app (run on a Mac; needs Xcode, no Docker)
  package linux        build dist/IRCord-GPUI-Linux-x64.deb in the build container
  package windows      build dist/IRCord-GPUI-Windows-x64-setup.exe in the build container
  package macos        build the .dmg and the updater's .app.tar.gz in dist/ (on a Mac)
  check                fmt, clippy, contract check and tests: what CI runs on a pull request
  ladder               the whole local ladder: check, then both installers
  release-assets <version> <installers> <out> <owner/repo>
                       sign the installers, write latest.json and SHA256SUMS (publish job)
  image-tag            print the build image's name (CI builds the image under it)
  integration          run the client against a real backend in disposable containers
  capture              screenshot the Linux app against a disposable backend (target/capture/)
  shell -- <cmd...>    run a command in the build container (e.g. cargo test -p ircord-client)
";

fn main() -> Result<()> {
    let root = repo_root();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["contract"] => contract::run(&root, false),
        ["contract", "--check"] => contract::run(&root, true),
        ["build", "windows"] => container::build_windows(&root),
        ["build", "linux"] => container::build_linux(&root),
        ["build", "macos"] => macos::build(&root).map(drop),
        ["package", "linux"] => package::linux(&root),
        ["package", "windows"] => package::windows(&root),
        ["package", "macos"] => package::macos(&root),
        ["check"] => check(&root),
        ["ladder"] => ladder(&root),
        ["release-assets", version, installers, out, repo] => release::run(
            &root,
            version,
            std::path::Path::new(installers),
            std::path::Path::new(out),
            repo,
        ),
        ["image-tag"] => {
            println!("{}", container::image_tag(&root)?);
            Ok(())
        }
        ["integration"] => integration::run(&root),
        ["capture"] => capture::run(&root),
        ["shell", "--", rest @ ..] if !rest.is_empty() => container::run(&root, rest),
        _ => {
            eprint!("{USAGE}");
            bail!("unknown command")
        }
    }
}

fn ladder(root: &std::path::Path) -> Result<()> {
    check(root)?;
    step("cargo xtask package windows");
    package::windows(root)?;
    step("cargo xtask package linux");
    package::linux(root)?;
    println!("\nladder passed");
    Ok(())
}

/// Ladder steps 1 to 3.
fn check(root: &std::path::Path) -> Result<()> {
    step("cargo fmt --check");
    container::run(root, &["cargo", "fmt", "--all", "--check"])?;
    step("cargo clippy");
    container::run(
        root,
        &[
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    step("cargo clippy for macOS and Windows (ircord-platform)");
    container::run(
        root,
        &[
            "cargo",
            "clippy",
            "--target",
            "aarch64-apple-darwin",
            "-p",
            "ircord-platform",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    container::run(
        root,
        &[
            "cargo",
            "xwin",
            "clippy",
            "--target",
            container::WINDOWS_TARGET,
            "-p",
            "ircord-platform",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    step("cargo xtask contract --check");
    contract::run(root, true)?;
    step("cargo test --workspace");
    container::run(root, &["cargo", "test", "--workspace"])?;
    println!("\ncheck passed");
    Ok(())
}

fn step(name: &str) {
    println!("\n==> {name}");
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository root")
        .to_path_buf()
}
