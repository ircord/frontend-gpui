//! `cargo xtask integration`: the client against a real backend.
//!
//! Builds the backend image from the sibling checkout, starts it and an Ergo
//! IRC server on a private Docker network, runs `crates/client/tests/live.rs`
//! in the build container sharing the backend's network namespace (so the
//! backend is `http://127.0.0.1:8080`, an address the client accepts), and
//! removes everything it started, pass or fail.
//!
//! Every live case reaches both servers from one address, so the servers'
//! per-address limits are lifted for this disposable pair: Ergo's connection
//! count and throttle through an environment override, and the backend's
//! sign-in budget (a fixed 30 a minute per client) by putting a proxy at
//! `127.0.0.1:8080` that the backend trusts (`IRCORD_TRUSTED_PROXIES`) and
//! that names each connection as its own client. The backend itself listens
//! on `127.0.0.1:8081` behind it.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Ergo image the backend's own integration checks use.
const ERGO_IMAGE: &str =
    "ghcr.io/ergochat/ergo@sha256:ef885e44f7fa19101bbbc41baef11dc280dc8107465dccaf6f0860f41b48a682";
const BACKEND_IMAGE: &str = "ircord-gpui-it-backend:local";
/// The proxy in front of the backend.
const PROXY_IMAGE: &str =
    "nginx@sha256:65645c7bb6a0661892a8b03b89d0743208a18dd2f3f17a54ef4b76fb8e2f2a10";

/// Forwards `127.0.0.1:8080` to the backend with `X-Forwarded-For` set to
/// `fd00:<connection>::1`: an address per connection, each in its own /64
/// (the backend counts IPv6 clients per /64). Sockets and uploads pass
/// through unbuffered.
const PROXY_CONF: &str = r#"worker_processes 1;
events {}
http {
    map $http_upgrade $connection_upgrade { default upgrade; '' close; }
    server {
        listen 127.0.0.1:8080;
        client_max_body_size 0;
        location / {
            proxy_pass http://127.0.0.1:8081;
            proxy_http_version 1.1;
            proxy_set_header Host $http_host;
            proxy_set_header X-Forwarded-For "fd00:${connection}::1";
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection $connection_upgrade;
            proxy_buffering off;
            proxy_request_buffering off;
            proxy_read_timeout 1h;
            proxy_send_timeout 1h;
        }
    }
}
"#;

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

/// A disposable backend and Ergo IRC server on a private Docker network,
/// removed when dropped. `server` and `ergo` are the container names; a
/// container started with `--network container:{server}` reaches the backend
/// at `http://127.0.0.1:8080` and the IRC server at `{ergo}:6667`.
pub struct Backend {
    pub server: String,
    pub ergo: String,
    proxy: String,
    _cleanup: Cleanup,
}

impl Backend {
    /// Build the backend image from `IRCORD_BACKEND` and start both servers.
    pub fn start(root: &Path) -> Result<Self> {
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
        docker(&[
            "run",
            "-d",
            "--name",
            &ergo,
            "--network",
            &net,
            // The whole block: Ergo cannot resolve an override of one of its keys.
            "-e",
            "ERGO__SERVER__IP_LIMITS={count: false, throttle: false}",
            ERGO_IMAGE,
        ])?;
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
            "IRCORD_LISTEN=127.0.0.1:8081",
            "-e",
            "IRCORD_PUBLIC_URL=http://127.0.0.1:8080",
            "-e",
            "IRCORD_ALLOW_PRIVATE_IRC=true",
            "-e",
            "IRCORD_YTDLP_BINARY=disabled",
            "-e",
            "IRCORD_TRUSTED_PROXIES=127.0.0.1/32",
            BACKEND_IMAGE,
        ])?;
        cleanup
            .0
            .push(vec!["rm".into(), "-f".into(), server.clone()]);
        let proxy = format!("ircord-gpui-it-proxy-{id}");
        docker(&[
            "run",
            "-d",
            "--name",
            &proxy,
            "--network",
            &format!("container:{server}"),
            "-e",
            &format!("IT_PROXY_CONF={PROXY_CONF}"),
            "--entrypoint",
            "sh",
            PROXY_IMAGE,
            "-c",
            r#"printf '%s' "$IT_PROXY_CONF" > /etc/nginx/nginx.conf && exec nginx -g 'daemon off;'"#,
        ])?;
        cleanup
            .0
            .push(vec!["rm".into(), "-f".into(), proxy.clone()]);
        Ok(Self {
            server,
            ergo,
            proxy,
            _cleanup: cleanup,
        })
    }

    /// `docker run` arguments that put a container in the backend's network
    /// namespace.
    pub fn network_args(&self) -> Vec<String> {
        vec!["--network".into(), format!("container:{}", self.server)]
    }

    /// Print the tail of the servers' logs, for a failed run.
    pub fn print_logs(&self) {
        eprintln!("--- backend log ---");
        let _ = Command::new("docker")
            .args(["logs", "--tail", "60", &self.server])
            .status();
        eprintln!("--- ergo log ---");
        let _ = Command::new("docker")
            .args(["logs", "--tail", "30", &self.ergo])
            .status();
        eprintln!("--- proxy log ---");
        let _ = Command::new("docker")
            .args(["logs", "--tail", "20", &self.proxy])
            .status();
    }
}

pub fn run(root: &Path) -> Result<()> {
    let backend = Backend::start(root)?;
    let mut extra = backend.network_args();
    extra.extend([
        "-e".into(),
        "IRCORD_IT_SERVER=http://127.0.0.1:8080".into(),
        "-e".into(),
        format!("IRCORD_IT_IRC={}:6667", backend.ergo),
    ]);
    let result = crate::container::run_with(
        root,
        &extra,
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
        backend.print_logs();
    }
    drop(backend);
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
