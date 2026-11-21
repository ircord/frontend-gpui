# Project agent memory

IRCord's native desktop client in Rust and GPUI (through `gpui-kit`). It talks
to the IRCord Go backend over the same HTTP and WebSocket contract as the web
client; it never speaks IRC itself. Layout and commands are in
[README.md](README.md).

## Sharp edges

- This machine has no C toolchain or GPUI system libraries. Every compile,
  clippy and test run goes through the build container:
  `cargo xtask shell -- cargo test -p ircord-client`. Only `cargo fmt` and
  `cargo xtask contract` run on the host. macOS cannot be built here at all;
  `cargo xtask build macos` is for a person on a Mac.
- To look at the app without a screen, `cargo xtask capture` runs the Linux
  build against a disposable backend and leaves screenshots in
  `target/capture/`. Your own Xvfb must run with `-extension COMPOSITE`, or
  every capture is black (see `xtask/src/capture.rs`).
- `crates/client` must not depend on GPUI: everything except drawing is
  tested there, without a window.
- `crates/contract/src/generated.rs` is generated from every schema in the
  pinned contract; change it only through `xtask/src/contract.rs` and
  `cargo xtask contract`. The socket envelope in `crates/contract/src/event.rs`
  is hand-written on purpose so unknown events decode as `Unknown`; a backend
  bump that adds an event type fails `crates/contract/tests/events.rs`.
- A feature adds no method to `client.rs` and no field to `Workspace` or
  `Changes`: it fills its pre-created files under `crates/client/src/api/` and
  `store/`, runs requests with `Client::call`, and tests against
  `ircord_client::testing::CannedServer` (feature `test-support`). Gate features on the server's
  `features` and `capabilities`, never on a version number.
- Your own messages carry your member ID as `senderId`, not `you`; use
  `Workspace::is_self`.
- `vendor/gpui-pre-windows` is GPUI's Windows backend with
  `vendor/gpui-pre-windows.patch` applied, so the `.exe` cross-builds on Linux.
  Read [vendor/README.md](vendor/README.md) before bumping `gpui-kit`.
- The sign-in lives in the OS credential store; there is deliberately no file
  fallback. A Linux session without a keyring service shows the error.

## Delivery

The repository is local and private with no remote and no CI, so a change is
delivered by passing this ladder in order and then landing on `main` locally.
`cargo xtask ladder` runs steps 1 to 4.

1. `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`.
2. `cargo xtask contract --check`: the committed contract matches the backend
   checkout it names (`IRCORD_BACKEND`, default `../ircord-backend`).
3. `cargo test --workspace`: contract, client and headless UI tests.
4. `cargo xtask build windows` and `cargo xtask build linux`: `dist/ircord.exe`
   and the Linux tarball exist.
5. For changes to the client's networking, `cargo xtask integration`: the
   client against a real backend and IRC server in disposable containers.
6. For a change a user can see: run it against a disposable local backend
   (`cargo xtask capture` on Linux) and say in the change note what was
   checked by eye and on which platform. A change nobody has run on Windows
   says so.
7. One line under `## Unreleased` in `CHANGELOG.md` for user-visible changes,
   in the style the other IRCord repositories use.
8. Land on `main` locally. Nothing is pushed and no pull request is opened
   until a remote exists.
9. Bumping `gpui-kit` is its own change: re-apply the Windows patch, run the
   whole ladder, and have the result run once on Windows before it lands.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
