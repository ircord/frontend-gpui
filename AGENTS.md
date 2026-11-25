# Project agent memory

IRCord's native desktop client in Rust and GPUI (through `gpui-kit`). It talks
to the IRCord Go backend over the same HTTP and WebSocket contract as the web
client; it never speaks IRC itself. Layout and commands are in
[README.md](README.md).

## Sharp edges

- This machine has no C toolchain or GPUI system libraries. Every compile,
  clippy and test run goes through the build container:
  `cargo xtask shell -- cargo test -p ircord-client`. Only `cargo fmt` and
  `cargo xtask contract` run on the host. macOS cannot be built here at all:
  `cargo xtask build macos` and `package macos` run on a Mac, and CI's
  `macos-arm64` job (`.github/workflows/build-installers.yaml`) is the only
  Mac build anyone sees on a pull request.
- To look at the app without a screen, `cargo xtask capture` runs the Linux
  build against a disposable backend and leaves screenshots in
  `target/capture/`. Your own Xvfb must run with `-extension COMPOSITE`, or
  every capture is black (see `xtask/src/capture.rs`, whose header lists the
  `CAPTURE_ONLY` scenes: `fade`, `news`, `attract`, `theme`).
- A theme's background picture is `theme::backdrop` (read with the session, decoded once, floors from `store::appearance::theme_backdrop`); a surface it shows through takes its colour from `backdrop::shell`/`get`, never `theme.sidebar` directly. `CAPTURE_ONLY=theme` and `CAPTURE_THEME=<dir with theme.json and picture.*>` narrow `cargo xtask capture` to that scene. `CAPTURE_ONLY=fade` captures a theme change fading (a few frames, light to dark).
- A theme change fades in over 200 ms (`theme::fade`): `theme::get` holds in-between colours meanwhile, so a UI test that reads colours after an edit, or after the account's look arrives, waits for `!theme::fade::active(cx)`; `theme::set` snaps. Anything costly per colour set (diagrams, the ambient light) reads `theme::settled`, the final colours.
- `crates/client` must not depend on GPUI: everything except drawing is
  tested there, without a window.
- Anything that calls the operating system for sound, video or
  notifications lives in `crates/platform` (no GPUI). The ladder clippies it
  for macOS and Windows too, the only compile gate for code nobody here can
  run. Linux video is the `ircord-video` helper (`crates/video-gst`), so the
  app itself never links GStreamer. A UI test that needs sound calls
  `media::sound::use_null`; the fixture opens no device.
- Voice calls are `crates/call` (LiveKit, whose prebuilt libwebrtc needs the
  image's clang 21); the ladder checks it for Windows only, and CI's Mac job
  is its only macOS build. The call's rules are `ircord_client::voice::Voice`;
  a UI test installs `testing::voice::FakeConnector` with
  `features::voice::use_connector`, never LiveKit.
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
- In `crates/app` a feature edits no shared view: it fills its pre-created
  `features/<name>.rs` (or `settings/<name>.rs`) and adds menu items, slots,
  panes, panels and keys through `registry.rs`. A menu item's `.order(…)` comes from
  `menu_order.rs`, which keeps every menu in the web client's order; place a
  new entry there rather than picking a number. Every string a person reads
  goes through `t!("id")`, with the text in its own
  `crates/app/i18n/en/<area>.ftl` (IDs start with the file name; the `i18n`
  tests fail on a missing or unused ID). Colours come from `theme::get(cx)`.
  A person's picture and name colour come from `ui::avatar::person` and
  `ui::avatar::name_color`: pictures load only from the signed-in server
  (`people::picture_url`, the web's #278 privacy rule), so never `img()` an
  `avatar_url` directly. `img(url)` sends no session: a picture under
  `/api/v1/` (Discover's previews) is read through the `Api` and drawn from
  bytes, as `settings/emotes.rs` does.
  A control drawn from a `div` takes `.reachable(&theme)` (`ui/focus.rs`)
  after `.test_support()`, so Tab reaches it; `tests/ui/audit.rs` fails on
  any shown control without a role, a name or keyboard reach.
  UI tests share `tests/ui/fixture.rs`, which runs the app against the
  `CannedServer`; wait with `fixture::wait`, not one `run_until_parked`.
  `window.click` refuses an element scrolled out of view: scroll its
  container with `window.scroll` first.
  Composer slots and suggestion providers run while the composer is being
  updated: read its text and caret with `composer::typed`, never
  `ctx.composer.read`. A view a slash command opens (`/list`, `/server`,
  `/gif`…) is wired with `features::commands::on_view`. Never name the type
  `.test_support()` returns in a signature: it exists only in test builds, so
  the release (`.exe`) build fails; return `Stateful<Div>` or
  `impl IntoElement` and call `.test_support()` at the use.
  Enter in a dialog's field also fires the dialog's Confirm, which closes
  it: a form that submits on Enter gives the dialog `.on_ok(|_, _, _| false)`.
  Icons: the app embeds only gpui-kit's default set (`ui::icon::IconName`);
  a Lucide-only `gpui_kit::assets::IconName` compiles but draws nothing.
  GPUI timers never fire in the headless tests: a debounce sleeps on the
  session's tokio runtime (`Session::runtime`), as `features/links.rs` does.
  A full page is a `ui::page::Page` (description, count chip, close, key
  hints; its hints name only keys the page handles). Escape is bound app-wide
  to Mark read, whose action runs before any `on_key_down`: a page that
  leaves on Escape captures `shortcuts::MarkRead`, as `features/inbox.rs` does.
- A live case that needs a server owner calls `support::make_owner`; `cargo
  xtask integration` gives the backend an operator token for it.
- Your own messages carry your member ID as `senderId`, not `you`; use
  `Workspace::is_self`.
- `vendor/gpui-pre-windows` is GPUI's Windows backend with
  `vendor/gpui-pre-windows.patch` applied, so the `.exe` cross-builds on Linux.
  Read [vendor/README.md](vendor/README.md) before bumping `gpui-kit`.
- The sign-in lives in the OS credential store; there is deliberately no file
  fallback. A Linux session without a keyring service shows the error.

## Delivery

Changes are delivered by pull request on `ircord/frontend-gpui`. Pass this
ladder locally before pushing, push the branch once, and open the pull request
as a draft. The ladder is the gate: a draft runs no CI. Marking it ready runs
`check.yaml` (steps 1 to 3), and `build-installers.yaml` as well when it
touches a path under `pull_request: paths:` in that workflow, the one list
(`cargo xtask ladder` reads it too). Keep the pull request a draft until the maintainer
marks it ready for review. `cargo xtask ladder` runs steps 1 to 4,
`cargo xtask check` steps 1 to 3.

1. `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`.
2. `cargo xtask contract --check`: the committed contract matches the backend
   checkout it names (`IRCORD_BACKEND`, default `../ircord-backend`).
3. `cargo test --workspace`: contract, client and headless UI tests.
4. `cargo check -p ircord-app --bin ircord` and the same with `cargo xwin
   check` for Windows: the app compiles without test features and for
   Windows. When the change against `main` touches a path that makes CI build
   the installers (the `pull_request` paths in `build-installers.yaml`),
   `cargo xtask package windows` and `cargo xtask package linux` as well: the
   setup `.exe` and the `.deb` exist in `dist/`. `cargo xtask ladder` decides.
5. For changes to the client's networking, `cargo xtask integration`: the
   client against a real backend and IRC server in disposable containers.
6. For a change a user can see: run it against a disposable local backend
   (`cargo xtask capture` on Linux) and say in the change note what was
   checked by eye and on which platform. A change nobody has run on Windows
   says so.
7. One line under `## Unreleased` in `CHANGELOG.md` for user-visible changes,
   in the style the other IRCord repositories use.
8. Push the branch once, with the ladder green on that exact commit, and open
   a draft pull request against `main`. Do not push further commits to a branch
   under review without a reason; each new commit restarts review.
9. Bumping `gpui-kit` is its own change: re-apply the Windows patch, run the
   whole ladder, and have the result run once on Windows before it lands.

A release is `publish.yaml`, started by hand (README.md, "Publishing a
release"); it is the only job that sees the update signing key.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
