# IRCord desktop (GPUI)

A native IRCord client written in Rust with [GPUI](https://www.gpui.rs/),
through [`gpui-kit`](https://crates.io/crates/gpui-kit). It replaces a web view
in a container app with native views, and talks to the IRCord backend over the
same HTTP and WebSocket contract as the web client.

It runs on Windows (cross-built on Linux), Linux, and macOS (built on a Mac).

## Layout

```
crates/contract   types generated from the pinned openapi.json, and the socket envelope
crates/client     HTTP, the event socket, the state store, persistence. No GPUI.
crates/app        GPUI views and the `ircord` binary
xtask/            contract refresh, the build container, builds, integration test, capture
vendor/           GPUI's Windows backend with the cross-build patch
tools/            the build container's Dockerfile
packaging/        icons, the Linux desktop entry and installer, the macOS Info.plist
```

## Commands

Everything that compiles runs in a Docker image built from
`tools/build.Dockerfile` on first use; the host needs Rust, git and Docker.

| Command | What it does |
| --- | --- |
| `cargo xtask ladder` | fmt, clippy, contract check, tests, and the Windows and Linux builds |
| `cargo xtask build windows` | cross-build `dist/ircord.exe`, with its icon and version |
| `cargo xtask build linux` | build `dist/ircord` and `dist/ircord-linux-x86_64.tar.gz` |
| `cargo xtask build macos` | on a Mac only: build `dist/IRCord.app` (see below) |
| `cargo xtask contract` | refresh `crates/contract` from the backend checkout |
| `cargo xtask contract --check` | fail if the committed contract is stale |
| `cargo xtask integration` | the client against a real backend and IRC server in containers |
| `cargo xtask capture` | screenshot the Linux app against a disposable backend, in `target/capture/` |
| `cargo xtask shell -- <cmd>` | run any command in the build container |

The backend checkout is `../ircord-backend` unless `IRCORD_BACKEND` names
another. The first Windows build downloads Microsoft's C runtime and Windows
SDK libraries (about 1 GB, cached in `~/.cache/cargo-xwin`); `cargo-xwin`
accepts Microsoft's licence for them. They are used for linking only.

## Running it

**Windows 10 or 11.** Copy `dist/ircord.exe` over and run it; nothing is
installed. Settings live in `%APPDATA%\com.ircord.gpui` (the server address,
window size and `ircord.log`); the sign-in is kept in Windows Credential
Manager under `com.ircord.gpui`.

**Linux** (x86-64; X11 or Wayland). Unpack `dist/ircord-linux-x86_64.tar.gz`
and run `ircord/install.sh`, which puts `ircord` in `~/.local/bin` with a menu
entry and icon (`install.sh --uninstall` removes them). Settings live in
`~/.config/com.ircord.gpui`. The sign-in is kept in the Secret Service (GNOME
Keyring or KWallet); without one running, sign-in says so. The binary needs
Vulkan, which every current desktop has (Mesa's drivers, or a GPU vendor's).

**macOS** (11 or later) is built on a Mac, since GPUI's macOS backend needs
Apple's SDK and Metal compiler. With Xcode (not only the command line tools),
Rust from rustup, and this repository checked out:

```sh
cargo xtask build macos        # builds dist/IRCord.app for this Mac's architecture
open dist/IRCord.app
```

The bundle is signed ad hoc, not with a developer ID, so the first launch of a
copy downloaded or moved from elsewhere needs Control-click → Open (or
`xattr -dr com.apple.quarantine IRCord.app`). The sign-in is kept in the
login keychain. If the build stops at the Metal shader step, install the
toolchain with `xcodebuild -downloadComponent MetalToolchain` and run it again.

**The tray.** Closing the window keeps IRCord running in the tray (Windows),
the menu bar (macOS) or the system tray (Linux); the tray menu shows it again
or quits. On Linux the icon is a StatusNotifierItem, which KDE, Cinnamon, XFCE
and Ubuntu's GNOME show; plain GNOME needs the AppIndicator extension. With no
tray to show it, closing the window quits IRCord instead.

The icons in `packaging/` are drawn by `packaging/icon.py`; change the design
there and run it again.

For development, `IRCORD_SERVER=http://127.0.0.1:8080` overrides the saved
server and `RUST_LOG=info` turns on logging. `cargo xtask capture` runs the
Linux build on a virtual display against a disposable backend, signs in through
the form and saves screenshots, so a change can be looked at without a screen.
