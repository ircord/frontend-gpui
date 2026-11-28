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
//!
//! The backend gets a random operator token (`IRCORD_ADMIN_TOKEN`), handed to
//! the live cases as `IRCORD_IT_ADMIN_TOKEN`, so a case can make its own
//! account a server owner through `/api/v1/admin/owners`.
//!
//! `IRCORD_IT_E2EE=1` starts the pair for end-to-end encryption instead and
//! runs only its live case (`tests/live/e2ee.rs`): the backend gets
//! `IRCORD_E2EE=experimental` and Ergo as its associated server, so every
//! account gets the home network encryption works on. Ergo's API listens on
//! loopback only, which the backend accepts in plain HTTP, so the backend and
//! the proxy share Ergo's network namespace.
//!
//! With encryption a second backend joins them, as a second IRCord server
//! on the same IRC network (taliesin beside chat.ircord.com): its own
//! database, the same Ergo, listening on `127.0.0.1:8083` behind a proxy at
//! `127.0.0.1:8082`, handed to the live cases as `IRCORD_IT_SERVER_B`. The
//! live cases reach the IRC server as `127.0.0.1:6667` there.

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

/// [`PROXY_CONF`] for a backend on another pair of ports.
fn proxy_conf(listen: u16, backend: u16) -> String {
    PROXY_CONF
        .replace("127.0.0.1:8080", &format!("127.0.0.1:{listen}"))
        .replace("127.0.0.1:8081", &format!("127.0.0.1:{backend}"))
}

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
    /// The container whose network namespace the backend's clients join.
    ns: String,
    pub ergo: String,
    /// The operator token the backend accepts on `/api/v1/admin/*`.
    pub admin_token: String,
    proxy: String,
    /// With encryption: the second backend and its proxy.
    second: Option<(String, String)>,
    _cleanup: Cleanup,
}

impl Backend {
    /// Build the backend image from `IRCORD_BACKEND` and start both servers;
    /// with `e2ee`, the backend has encryption on and Ergo as its associated
    /// server (see the module docs).
    pub fn start_with(root: &Path, e2ee: bool) -> Result<Self> {
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
        let admin_token = random_token()?;
        let admin_env = format!("IRCORD_ADMIN_TOKEN={admin_token}");
        let api_token = random_token()?;
        let ergo_api = format!(
            "ERGO__API={{enabled: true, listener: \"127.0.0.1:8089\", bearer-tokens: [{api_token}]}}"
        );
        let mut ergo_env: Vec<String> = Vec::new();
        let mut backend_env: Vec<String> = Vec::new();
        if e2ee {
            ergo_env.extend(["-e".into(), ergo_api]);
            for env in [
                "IRCORD_E2EE=experimental".to_string(),
                "IRCORD_ERGO_SERVER=127.0.0.1:6667".into(),
                "IRCORD_ERGO_API_URL=http://127.0.0.1:8089".into(),
                format!("IRCORD_ERGO_API_TOKEN={api_token}"),
                "IRCORD_ERGO_IRC_TLS=false".into(),
                // A second copy of the app signs in as `localhost`, so its
                // sign-in is kept apart from the first one's.
                "IRCORD_ALLOWED_ORIGINS=http://localhost:8080".into(),
            ] {
                backend_env.extend(["-e".into(), env]);
            }
        }
        // Where the backend and the proxy live: their own namespace on the
        // private network, or Ergo's.
        let place = if e2ee {
            format!("container:{ergo}")
        } else {
            net.clone()
        };

        docker(&["network", "create", &net])?;
        cleanup
            .0
            .push(vec!["network".into(), "rm".into(), net.clone()]);
        docker(
            &[
                "run",
                "-d",
                "--name",
                &ergo,
                "--network",
                &net,
                // The whole block: Ergo cannot resolve an override of one of its keys.
                "-e",
                "ERGO__SERVER__IP_LIMITS={count: false, throttle: false}",
            ]
            .into_iter()
            .chain(ergo_env.iter().map(String::as_str))
            .chain([ERGO_IMAGE])
            .collect::<Vec<_>>(),
        )?;
        cleanup.0.push(vec!["rm".into(), "-f".into(), ergo.clone()]);
        let backend_args: Vec<&str> = vec![
            "run",
            "-d",
            "--name",
            &server,
            "--network",
            &place,
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
            "-e",
            &admin_env,
        ];
        docker(
            &backend_args
                .into_iter()
                .chain(backend_env.iter().map(String::as_str))
                .chain([BACKEND_IMAGE])
                .collect::<Vec<_>>(),
        )?;
        cleanup
            .0
            .push(vec!["rm".into(), "-f".into(), server.clone()]);
        // The namespace every other container joins.
        let ns = if e2ee { ergo.clone() } else { server.clone() };
        let start_proxy = |name: &str, conf: &str, cleanup: &mut Cleanup| -> Result<()> {
            docker(&[
                "run",
                "-d",
                "--name",
                name,
                "--network",
                &format!("container:{ns}"),
                "-e",
                &format!("IT_PROXY_CONF={conf}"),
                "--entrypoint",
                "sh",
                PROXY_IMAGE,
                "-c",
                r#"printf '%s' "$IT_PROXY_CONF" > /etc/nginx/nginx.conf && exec nginx -g 'daemon off;'"#,
            ])?;
            cleanup
                .0
                .push(vec!["rm".into(), "-f".into(), name.to_string()]);
            Ok(())
        };
        let proxy = format!("ircord-gpui-it-proxy-{id}");
        start_proxy(&proxy, PROXY_CONF, &mut cleanup)?;
        let second = if e2ee {
            // A second IRCord server on the same IRC network.
            let server_b = format!("ircord-gpui-it-backend-b-{id}");
            let args: Vec<String> = [
                "run",
                "-d",
                "--name",
                &server_b,
                "--network",
                &place,
                "--tmpfs",
                "/data:uid=65532,gid=65532",
                "-e",
                "IRCORD_LISTEN=127.0.0.1:8083",
                "-e",
                "IRCORD_PUBLIC_URL=http://127.0.0.1:8082",
                "-e",
                "IRCORD_ALLOW_PRIVATE_IRC=true",
                "-e",
                "IRCORD_YTDLP_BINARY=disabled",
                "-e",
                "IRCORD_TRUSTED_PROXIES=127.0.0.1/32",
                "-e",
                &admin_env,
            ]
            .into_iter()
            .map(str::to_string)
            .chain(
                backend_env
                    .iter()
                    .map(|e| e.replace("http://localhost:8080", "http://localhost:8082")),
            )
            .chain([BACKEND_IMAGE.to_string()])
            .collect();
            docker(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
            cleanup
                .0
                .push(vec!["rm".into(), "-f".into(), server_b.clone()]);
            let proxy_b = format!("ircord-gpui-it-proxy-b-{id}");
            start_proxy(&proxy_b, &proxy_conf(8082, 8083), &mut cleanup)?;
            Some((server_b, proxy_b))
        } else {
            None
        };
        Ok(Self {
            server,
            ns,
            ergo,
            admin_token,
            proxy,
            second,
            _cleanup: cleanup,
        })
    }

    /// `docker run` arguments that put a container in the backend's network
    /// namespace.
    pub fn network_args(&self) -> Vec<String> {
        vec!["--network".into(), format!("container:{}", self.ns)]
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
        if let Some((server_b, _)) = &self.second {
            eprintln!("--- second backend log ---");
            let _ = Command::new("docker")
                .args(["logs", "--tail", "60", server_b])
                .status();
        }
    }
}

pub fn run(root: &Path) -> Result<()> {
    let e2ee = std::env::var_os("IRCORD_IT_E2EE").is_some();
    let backend = Backend::start_with(root, e2ee)?;
    let mut extra = backend.network_args();
    if e2ee {
        extra.extend([
            "-e".into(),
            "IRCORD_IT_E2EE=1".into(),
            "-e".into(),
            "IRCORD_IT_SERVER_B=http://127.0.0.1:8082".into(),
            "-e".into(),
            "IRCORD_IT_IRC_FROM_TEST=127.0.0.1:6667".into(),
        ]);
    }
    extra.extend([
        "-e".into(),
        "IRCORD_IT_SERVER=http://127.0.0.1:8080".into(),
        "-e".into(),
        format!("IRCORD_IT_IRC={}:6667", backend.ergo),
        "-e".into(),
        format!("IRCORD_IT_ADMIN_TOKEN={}", backend.admin_token),
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
        ]
        .into_iter()
        // With encryption on, every account has the home network too, which
        // the other cases do not expect: only the encryption case runs.
        .chain(e2ee.then_some("e2ee"))
        .collect::<Vec<_>>(),
    );
    if result.is_err() {
        backend.print_logs();
    }
    drop(backend);
    result.context("the live integration test failed")?;
    println!("integration passed");
    Ok(())
}

/// 32 random bytes as 64 hex digits, the form the backend's admin token takes.
fn random_token() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .context("read /dev/urandom for the admin token")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
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
