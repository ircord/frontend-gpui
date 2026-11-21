# IRCord desktop (GPUI)

A native IRCord client written in Rust with [GPUI](https://www.gpui.rs/),
through [`gpui-kit`](https://crates.io/crates/gpui-kit). It replaces a web view
in a container app with native views, and talks to the IRCord backend over the
same HTTP and WebSocket contract as the web client.

The first version targets Windows, cross-built on Linux. Linux and macOS builds
come later.

## Layout

```
crates/contract   types generated from the pinned openapi.json, and the socket envelope
crates/client     HTTP, the event socket, the state store, persistence. No GPUI.
crates/app        GPUI views and the `ircord` binary
xtask/            contract refresh, the build container, cross-builds, integration test
vendor/           GPUI's Windows backend with the cross-build patch
tools/            the build container's Dockerfile
```

## Commands

Everything that compiles runs in a Docker image built from
`tools/build.Dockerfile` on first use; the host needs Rust, git and Docker.

| Command | What it does |
| --- | --- |
| `cargo xtask ladder` | fmt, clippy, contract check, tests and the Windows build |
| `cargo xtask build windows` | cross-build `dist/ircord.exe` |
| `cargo xtask build linux` | build `dist/ircord` for Linux |
| `cargo xtask contract` | refresh `crates/contract` from the backend checkout |
| `cargo xtask contract --check` | fail if the committed contract is stale |
| `cargo xtask integration` | the client against a real backend and IRC server in containers |
| `cargo xtask shell -- <cmd>` | run any command in the build container |

The backend checkout is `../ircord-backend` unless `IRCORD_BACKEND` names
another. The first Windows build downloads Microsoft's C runtime and Windows
SDK libraries (about 1 GB, cached in `~/.cache/cargo-xwin`); `cargo-xwin`
accepts Microsoft's licence for them. They are used for linking only.

## Running it

Copy `dist/ircord.exe` to a Windows 10 or 11 machine and run it. Nothing is
installed. Settings live in `%APPDATA%\com.ircord.gpui` (the server address,
window size and `ircord.log`); the sign-in is kept in Windows Credential
Manager under `com.ircord.gpui`. Closing the window keeps IRCord in the tray;
use the tray menu to show it again or quit.

For development, `IRCORD_SERVER=http://127.0.0.1:8080` overrides the saved
server and `RUST_LOG=info` turns on logging.
