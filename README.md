# IRCord GPUI

> **Experimental.** This is a work-in-progress native client, not ready for
> daily use. It is not the supported desktop client: for that, use
> [ircord/desktop](https://github.com/ircord/desktop).

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
crates/platform   what the app asks of the OS besides drawing: sound, video, notifications. No GPUI.
crates/video-gst  `ircord-video`, the Linux video helper: GStreamer in its own process
xtask/            contract refresh, the build container, builds, integration test, capture
vendor/           GPUI's Windows backend with the cross-build patch
tools/            the build container's Dockerfile
bin/dev.sh        cargo in the build container
packaging/        icons, the Linux desktop entry and installer, the macOS Info.plist,
                  the installer config (Packager.toml) and the update key (updater.pub)
.github/          CI: checks on pull requests, the installers, the release
```

## Commands

Everything that compiles runs in a Docker image built from
`tools/build.Dockerfile` on first use; the host needs Rust, git and Docker.

| Command | What it does |
| --- | --- |
| `cargo xtask ladder` | `check`, compile checks of the app for Linux and Windows, then the Windows and Linux installers when the change touches a path that makes CI build them |
| `cargo xtask check` | fmt, clippy (and `crates/platform` for macOS and Windows), contract check and tests: what CI runs on a pull request |
| `cargo xtask build windows` | cross-build `dist/ircord.exe`, with its icon and version |
| `cargo xtask build linux` | build `dist/ircord`, `dist/ircord-video` and `dist/ircord-linux-x86_64.tar.gz` |
| `cargo xtask build macos` | on a Mac only: build `dist/IRCord GPUI.app` (see below) |
| `cargo xtask dev` | run the native app and apply supported Rust edits while it is running |
| `cargo xtask package windows` | the Windows build, then its setup, `dist/IRCord-GPUI-Windows-x64-setup.exe` |
| `cargo xtask package linux` | the Linux build, then `dist/IRCord-GPUI-Linux-x64.deb` |
| `cargo xtask package macos` | on an Apple Silicon Mac: the bundle, `dist/IRCord-GPUI-macOS-Apple-Silicon.dmg` and the updater's `.app.tar.gz` |
| `cargo xtask release-assets …` | the release job's signing, `latest.json` and `SHA256SUMS` (see "Publishing a release") |
| `cargo xtask contract` | refresh `crates/contract` from the backend checkout |
| `cargo xtask contract --check` | fail if the committed contract is stale |
| `cargo xtask integration` | the client against a real backend and IRC server in containers |
| `cargo xtask capture` | screenshot the Linux app against a disposable backend, in `target/capture/` |
| `cargo xtask shell -- <cmd>` | run any command in the build container |
| `bin/dev.sh <cargo args>` | run cargo in the build container (`bin/dev.sh clippy --all-targets`) |

The backend checkout is `../ircord-backend` unless `IRCORD_BACKEND` names
another. The first Windows build downloads Microsoft's C runtime and Windows
SDK libraries (about 1 GB, cached in `~/.cache/cargo-xwin`); `cargo-xwin`
accepts Microsoft's licence for them. They are used for linking only.

Compiled dependencies are cached by sccache in `~/.cache/sccache` (capped at
10 GB), shared by every checkout on the machine, so a checkout that has never
built, or one after a dependency bump, reuses what another checkout compiled;
proc macros, build scripts and the workspace crates still compile. A checkout
whose `target/` is up to date gains nothing from it. CI does not use it.

## Running it

### Development with hot reload

Install [Dioxus CLI 0.7.10](https://github.com/DioxusLabs/dioxus/releases/tag/v0.7.10)
and run `cargo xtask dev` from the repository root. If multiple tools supply
`dx`, set `IRCORD_DX` to the absolute path of the Dioxus executable.
Leave the dev command running while editing Rust code.

The development launcher has its own package so the watcher recompiles the
UI library for a hot patch. GPUI applies compatible view changes in the
running app. Changes to types, native window setup and controls registered
at startup can require a restart. In the interactive terminal, press `r`
to force a full build. Use `cargo xtask dev --restart` to rebuild and
replace the app on each edit. Press Ctrl+C to stop the watcher and app.

On macOS, sign in once in the dev client. It keeps its own session in a private
SQLite database under `~/Library/Application Support/com.ircord.gpui/development`.
The installed client keeps its session in Keychain.

The watcher uses a valid `IRCord Local Development` code-signing identity when
present. Set `IRCORD_DEV_SIGNING_IDENTITY` to use a different identity by name
or fingerprint. The signed watcher rebuilds and restarts the app at a fixed
bundle path. Dioxus 0.7.10 writes a web page into the bundle when it applies a
hot patch, which invalidates the bundle signature.

### Installers

Releases on GitHub will carry the installers below once the first is
published. Without a code-signing certificate, Windows warns about an unknown
publisher on the first run of the setup ("More info", then "Run anyway"), and
macOS asks once as described under macOS.

**Windows 10 or 11.** Run `IRCord-GPUI-Windows-x64-setup.exe`; it installs
for the current user, with a Start menu entry, and needs no administrator.
The setup also makes IRCord GPUI the app for `irc://` and `ircs://` links; to
give them back to another IRC app, set that app as the handler again in its
own settings (Settings → This device here shows which app has them), and
uninstalling IRCord GPUI removes its registration if it still points at it.
A build from this repository also runs as it is: copy `dist/ircord.exe` over
and run it. Settings live in `%APPDATA%\com.ircord.gpui` (the server address,
window size and `ircord.log`); the sign-in is kept in Windows Credential
Manager under `com.ircord.gpui`.

**Linux** (x86-64; X11 or Wayland). On Debian 12, Ubuntu 22.04 or later,
`sudo apt install ./IRCord-GPUI-Linux-x64.deb` installs `ircord`, its video
helper, the menu entry and the GStreamer plugins it plays videos with.
Elsewhere, unpack `dist/ircord-linux-x86_64.tar.gz` and run
`ircord/install.sh`, which puts `ircord` in `~/.local/bin` with a menu
entry and icon (`install.sh --uninstall` removes them). Settings live in
`~/.config/com.ircord.gpui`. The sign-in is kept in the Secret Service (GNOME
Keyring or KWallet); without one running, sign-in says so. The binary needs
Vulkan, which every current desktop has (Mesa's drivers, or a GPU vendor's).
Videos in messages play in the app when GStreamer and its base, good and libav
plugins are installed (Debian and Ubuntu: `gstreamer1.0-plugins-base
gstreamer1.0-plugins-good gstreamer1.0-libav`); `ircord-video`, installed
beside `ircord`, plays them. Without them a video's card opens the browser.

**macOS** (11 or later) is built on a Mac, since GPUI's macOS backend needs
Apple's SDK and Metal compiler. With Xcode (not only the command line tools),
Rust from rustup, and this repository checked out:

```sh
cargo xtask build macos          # builds dist/IRCord GPUI.app for this Mac's architecture
open "dist/IRCord GPUI.app"
```

`cargo xtask package macos` also makes the disk image a release ships.

The bundle is signed ad hoc, not with a developer ID, so the first launch of a
copy downloaded or moved from elsewhere needs Control-click → Open (or
`xattr -dr com.apple.quarantine "IRCord GPUI.app"`). The sign-in is kept in the
login keychain. If the build stops at the Metal shader step, install the
toolchain with `xcodebuild -downloadComponent MetalToolchain` and run it again.

**The tray.** Closing the window keeps IRCord running in the tray (Windows),
the menu bar (macOS) or the system tray (Linux); the tray menu shows it again
or quits. On Linux the icon is a StatusNotifierItem, which KDE, Cinnamon, XFCE
and Ubuntu's GNOME show; plain GNOME needs the AppIndicator extension. With no
tray to show it, closing the window quits IRCord instead.

**Keyboard and screen readers.** Tab and Shift+Tab move through every
control, Enter or Space runs the one with the ring, and Ctrl+/ lists the
other keys. IRCord exposes its controls to the system's accessibility
interface (UI Automation on Windows, AT-SPI on Linux, NSAccessibility on
macOS), and a screen reader hears new messages in the open conversation as
one short update every few seconds at most.

The icons in `packaging/` are drawn by `packaging/icon.py`; change the design
there and run it again.

For development, `IRCORD_SERVER=http://127.0.0.1:8080` overrides the saved
server and `RUST_LOG=info` turns on logging. `cargo xtask capture` runs the
Linux build on a virtual display against a disposable backend, signs in through
the form, types a server emoji into the message box and saves
screenshots, so a change can be looked at without a screen.

Press Shift+F8 to pause or resume a personal theme. To start with personal
themes paused, quit IRCord completely and pass `--safe-theme` to the app.
On macOS, use `open -a 'IRCord GPUI' --args --safe-theme`.

## Publishing a release

Pull requests run `.github/workflows/check.yaml` once they are marked ready
(drafts run nothing). Those that change how installers are made, or
`crates/platform`, also run `build-installers.yaml`, which builds the `.deb`,
cross-builds the Windows setup and installs and starts it on Windows, and
builds the Mac disk image on macOS.

A release is `.github/workflows/publish.yaml`, started by hand:

1. On `main`, set `version` under `[workspace.package]` in `Cargo.toml`
   (and `Cargo.lock` with it), and move the `## Unreleased` entries in
   `CHANGELOG.md` under `## v<version> — <date>`.
2. Run the workflow with that version and **verify only** ticked. It checks
   the version, runs the checks, builds every installer, signs the Windows
   setup and the macOS `.app.tar.gz` for the updater, writes `latest.json`
   and `SHA256SUMS`, and keeps them as the `ircord-gpui-release` artifact.
3. Run it again without **verify only** to tag `v<version>` and publish the
   release, with that version's changelog section as its notes.

The release holds `IRCord-GPUI-Windows-x64-setup.exe`,
`IRCord-GPUI-macOS-Apple-Silicon.dmg`, `IRCord-GPUI-Linux-x64.deb`, the
updater's `IRCord-GPUI-macOS-Apple-Silicon.app.tar.gz`, a `.sig` for each
update, `latest.json`, `build-info.txt` (the commit and the backend contract
it was built against) and `SHA256SUMS`.

Updates are signed with a minisign key in cargo-packager's format. The
repository's Actions secrets `CARGO_PACKAGER_SIGN_PRIVATE_KEY` and
`CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD` hold it, only the publish job sees
them, and `packaging/updater.pub` is its public half, the key installed
copies trust. Losing the key means installed copies can no longer update.

## License

MIT. See [LICENSE](LICENSE).
