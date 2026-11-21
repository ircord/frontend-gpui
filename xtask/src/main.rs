//! Repository tasks: `cargo xtask <command>`.
//!
//! GPUI needs a C toolchain and Linux system libraries, and the Windows build
//! needs clang, lld and the Microsoft libraries `cargo-xwin` fetches. All of it
//! runs inside the image built from `tools/build.Dockerfile`, so the only
//! things a host needs are Rust, git and Docker.

mod container;
mod contract;
mod integration;

use anyhow::{Result, bail};
use std::path::PathBuf;

const USAGE: &str = "\
usage: cargo xtask <command>

  contract [--check]   refresh (or check) crates/contract from the backend checkout
  build windows        cross-build dist/ircord.exe in the build container
  build linux          build dist/ircord for Linux in the build container
  ladder               run the whole local ladder (fmt, clippy, contract, tests, windows build)
  integration          run the client against a real backend in disposable containers
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
        ["ladder"] => ladder(&root),
        ["integration"] => integration::run(&root),
        ["shell", "--", rest @ ..] if !rest.is_empty() => container::run(&root, rest),
        _ => {
            eprint!("{USAGE}");
            bail!("unknown command")
        }
    }
}

fn ladder(root: &std::path::Path) -> Result<()> {
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
    step("cargo xtask contract --check");
    contract::run(root, true)?;
    step("cargo test --workspace");
    container::run(root, &["cargo", "test", "--workspace"])?;
    step("cargo xtask build windows");
    container::build_windows(root)?;
    println!("\nladder passed");
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
