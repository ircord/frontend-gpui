//! Desktop integration (package DK1):
//!
//! - **One running copy.** The first copy listens on a loopback port named
//!   in `instance` in the settings folder. A later copy hands its links to
//!   it over that port (with the token from the same file) and exits, and
//!   the first copy brings its window up and opens them ([`hand_over`],
//!   [`instance`]). macOS never starts a second copy: it hands links to the
//!   running app ([`open_urls`]).
//! - **Links.** `irc://` and `ircs://` links, and links to a message on the
//!   signed-in server, open in the app ([`open_link`]); links that arrive
//!   before the workspace has loaded wait for it. Registering IRCord as the
//!   system's handler for `irc://` is [`links`], shown in Settings → This
//!   device.
//! - **Tray mark.** The tray icon carries a red dot while mentions or direct
//!   messages wait, as the web client's app badge counts them ([`attention`]).
//! - **Window placement.** The main window opens where it was last, maximised
//!   if it was, as long as that place is still on a screen ([`placement`]).
//!
//! What this device keeps lives in the `desktop` section of the device state.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::*;
use ircord_client::config::ConfigDir;
use ircord_client::contract::{Conversation, ConversationKind};
use ircord_client::format::links::{network_name_from_host, parse_irc_url, parse_message_link};
use ircord_client::store::Workspace;
use ircord_client::store::sidebar_order::{self, Level};

use crate::features::{network_form, text};
use crate::session::{Phase, Session, SessionEvent};
use crate::{MainView, loading, tray, ui};

/// The device state section DK1 keeps.
pub const SECTION: &str = "desktop";

/// What this device keeps, as plain numbers so the section reads well in
/// `state.json`: the window's `x`, `y`, `width`, `height` and `maximized`
/// (1 or 0), and `trayMark` (0 turns the tray mark off).
pub type Saved = BTreeMap<String, f64>;

/// The app's own state: the session, links waiting for the workspace, and
/// the tray mark last drawn.
#[derive(Default)]
struct Desktop {
    session: Option<WeakEntity<Session>>,
    waiting: Vec<String>,
    /// The attention count the tray shows, once drawn.
    marked: Option<i64>,
    /// Whether the tray shows the mark, as saved on this device.
    mark_on: bool,
    /// The window bounds to save once moving and resizing pause.
    unsaved: Option<WindowBounds>,
}
impl Global for Desktop {}

pub fn init(cx: &mut App) {
    cx.default_global::<Desktop>();
    cx.observe_new::<Session>(|session: &mut Session, _, cx| {
        let weak = cx.weak_entity();
        let mark_on = tray_mark_on(&session.config.section(SECTION));
        let desktop = cx.global_mut::<Desktop>();
        desktop.session = Some(weak);
        desktop.mark_on = mark_on;
        cx.subscribe_self(|_, event: &SessionEvent, cx| match event {
            SessionEvent::Phase | SessionEvent::Changed(_) => {
                let session = cx.entity();
                cx.defer(move |cx| {
                    refresh_mark(&session, cx);
                    drain(&session, cx);
                });
            }
            _ => {}
        })
        .detach();
        // Links from a second copy or from the system, as they come.
        if let Some(mut inbox) = inbox() {
            cx.spawn(async move |this, cx| {
                while let Some(links) = inbox.next().await {
                    let alive = this.update(cx, |_, cx| {
                        let session = cx.entity();
                        cx.defer(move |cx| {
                            crate::show_main_window(session.clone(), cx);
                            receive(&session, links, cx);
                        });
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
    })
    .detach();
    // Remember where the main window is whenever it moves or resizes.
    cx.observe_new::<MainView>(|_, window, cx| {
        if let Some(window) = window {
            cx.observe_window_bounds(window, |_, window, cx| {
                remember(window.window_bounds(), cx);
            })
            .detach();
        }
    })
    .detach();
}

fn session(cx: &App) -> Option<Entity<Session>> {
    cx.try_global::<Desktop>()?.session.as_ref()?.upgrade()
}

// ---- Links ------------------------------------------------------------

/// The links among the program's arguments: `irc://`, `ircs://` and web
/// links (a message link is a web link on the server). Anything else, such
/// as a flag, is ignored.
pub fn links_from_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .map(|a| a.trim().to_string())
        .filter(|a| {
            let lower = a.to_ascii_lowercase();
            ["irc://", "ircs://", "https://", "http://"]
                .iter()
                .any(|scheme| lower.starts_with(scheme))
                && a.len() <= instance::MAX_LINE
                && !a.contains(['\n', '\r'])
        })
        .take(instance::MAX_LINKS)
        .collect()
}

/// Links arrived (from a second copy, the system or the command line): open
/// them once the workspace is there. An empty list only brings the window up.
pub fn receive(session: &Entity<Session>, links: Vec<String>, cx: &mut App) {
    cx.default_global::<Desktop>().waiting.extend(links);
    drain(session, cx);
}

/// Open the waiting links, if the workspace has loaded and a window shows.
fn drain(session: &Entity<Session>, cx: &mut App) {
    let ready = matches!(session.read(cx).phase, Phase::SignedIn { .. })
        && loading::bootstrapped(session.read(cx));
    if !ready || cx.global::<Desktop>().waiting.is_empty() {
        return;
    }
    let Some(window) = cx
        .active_window()
        .or_else(|| cx.windows().into_iter().next())
    else {
        return;
    };
    let links = std::mem::take(&mut cx.global_mut::<Desktop>().waiting);
    let session = session.clone();
    let opened = window.update(cx, |_, window, cx| {
        for link in &links {
            open_link(&session, link, window, cx);
        }
    });
    if opened.is_err() {
        cx.global_mut::<Desktop>().waiting = links;
    }
}

/// The network a link's host names: the same host, or the same network under
/// another host (`irc.libera.chat` and `libera.chat`).
pub fn network_for(w: &Workspace, host: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    let base = network_name_from_host(&host).to_string();
    w.networks
        .iter()
        .find(|n| n.host.eq_ignore_ascii_case(&host))
        .or_else(|| {
            w.networks.iter().find(|n| {
                network_name_from_host(&n.host.to_ascii_lowercase()).eq_ignore_ascii_case(&base)
            })
        })
        .map(|n| n.id.clone())
}

/// Open a link handed to the app: an `irc://` link joins (after asking) or
/// opens its channel, person or network; a link to a message opens its
/// conversation. An `irc://` link to a network you do not have opens the
/// form to add it.
pub fn open_link(session: &Entity<Session>, url: &str, window: &mut Window, cx: &mut App) {
    if let Some(link) = parse_irc_url(url) {
        let found = session
            .read(cx)
            .client()
            .and_then(|c| network_for(&c.read(), &link.host).map(|id| (c.clone(), id)));
        let Some((client, network)) = found else {
            ui::toast::info(window, cx, t!("desktop-link-new-network", host = link.host));
            network_form::open_add(session, window, cx);
            return;
        };
        if let Some(channel) = &link.channel {
            text::open_channel(session, &network, channel, window, cx);
        } else if let Some(nick) = &link.nick {
            // T1's opener finds the network by its exact host.
            let host = client
                .read()
                .network(&network)
                .map(|n| n.host.clone())
                .unwrap_or(link.host.clone());
            let url = format!("irc://{host}/{nick},isnick");
            text::open_irc_link(session, &url, window, cx);
        } else {
            session.update(cx, |s, cx| s.select_network(&network, cx));
        }
        return;
    }
    let origin = session.read(cx).server().map(|s| s.origin().to_string());
    match origin.and_then(|o| parse_message_link(url, &o)) {
        Some(link) => text::open_message_link(session, &link, window, cx),
        None => ui::toast::info(window, cx, t!("desktop-link-unknown")),
    }
}

/// Where links for the app wait until the session takes them.
struct Inbox {
    send: mpsc::UnboundedSender<Vec<String>>,
    take: Mutex<Option<mpsc::UnboundedReceiver<Vec<String>>>>,
}

fn inbox_cell() -> &'static Inbox {
    static INBOX: OnceLock<Inbox> = OnceLock::new();
    INBOX.get_or_init(|| {
        let (send, receive) = mpsc::unbounded();
        Inbox {
            send,
            take: Mutex::new(Some(receive)),
        }
    })
}

/// The receiving end, once per process (the first session takes it).
fn inbox() -> Option<mpsc::UnboundedReceiver<Vec<String>>> {
    inbox_cell().take.lock().ok()?.take()
}

fn deliver(links: Vec<String>) {
    let _ = inbox_cell().send.unbounded_send(links);
}

/// Before the app starts: hand this launch's links to a copy that is already
/// running and return `true` (this copy should exit), or become the running
/// copy and return `false`.
pub fn hand_over() -> bool {
    let links = links_from_args(std::env::args().skip(1));
    let claim = match ConfigDir::platform() {
        Ok(config) => instance::claim(config.path(), &links),
        Err(_) => instance::Claim::First(None),
    };
    match claim {
        instance::Claim::Handed => true,
        instance::Claim::First(listener) => {
            if let Some(listener) = listener {
                listener.serve(deliver);
            }
            if !links.is_empty() {
                deliver(links);
            }
            false
        }
    }
}

/// Links the system hands the running app (macOS opens `irc://` links this way).
pub fn open_urls(urls: Vec<String>) {
    deliver(links_from_args(urls));
}

/// The Dock icon was clicked while the app runs (macOS): show the window.
pub fn reopen(cx: &mut App) {
    if let Some(session) = session(cx) {
        crate::show_main_window(session, cx);
    }
}

/// The single running copy: a loopback listener and the file that names it.
pub mod instance {
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
    use std::path::Path;
    use std::time::Duration;

    /// The file in the settings folder: `<port> <token>`.
    pub const FILE: &str = "instance";
    const HELLO: &str = "ircord-handoff 1";
    pub const MAX_LINKS: usize = 32;
    pub const MAX_LINE: usize = 4096;
    const WAIT: Duration = Duration::from_secs(2);

    pub enum Claim {
        /// Another copy took the links; this one should exit.
        Handed,
        /// This is the running copy now, listening when it could.
        First(Option<Listener>),
    }

    pub struct Listener {
        socket: TcpListener,
        token: String,
    }

    /// Hand `links` to the copy named in `dir`, or become that copy.
    pub fn claim(dir: &Path, links: &[String]) -> Claim {
        if let Some((port, token)) = read(dir)
            && send(port, &token, links).is_ok()
        {
            return Claim::Handed;
        }
        match listen(dir) {
            Ok(listener) => Claim::First(Some(listener)),
            Err(e) => {
                log::warn!("single copy: {e}");
                Claim::First(None)
            }
        }
    }

    fn read(dir: &Path) -> Option<(u16, String)> {
        let text = std::fs::read_to_string(dir.join(FILE)).ok()?;
        let (port, token) = text.trim().split_once(' ')?;
        Some((port.parse().ok()?, token.to_string()))
    }

    fn send(port: u16, token: &str, links: &[String]) -> io::Result<()> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        let mut message = format!("{HELLO}\n{token}\n");
        for link in links {
            message.push_str(link);
            message.push('\n');
        }
        message.push('\n');
        stream.write_all(message.as_bytes())?;
        let mut reply = String::new();
        BufReader::new(stream.take(16)).read_line(&mut reply)?;
        if reply.trim() == "ok" {
            Ok(())
        } else {
            Err(io::Error::other("the running copy did not answer"))
        }
    }

    fn listen(dir: &Path) -> io::Result<Listener> {
        let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = socket.local_addr()?.port();
        let token = ircord_client::client::random_id();
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE);
        let staging = path.with_extension(format!("{}.tmp", std::process::id()));
        write_private(&staging, format!("{port} {token}\n").as_bytes())?;
        std::fs::rename(staging, path)?;
        Ok(Listener { socket, token })
    }

    #[cfg(unix)]
    fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)
    }

    #[cfg(not(unix))]
    fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
        std::fs::write(path, bytes)
    }

    impl Listener {
        /// Take hand-overs on a thread of their own, calling `deliver` with
        /// each one's links (none: just show the window).
        pub fn serve(self, deliver: impl Fn(Vec<String>) + Send + 'static) {
            let spawned = std::thread::Builder::new()
                .name("ircord-handoff".into())
                .spawn(move || {
                    for stream in self.socket.incoming().flatten() {
                        match accept(stream, &self.token) {
                            Ok(links) => deliver(links),
                            Err(e) => log::warn!("hand-over refused: {e}"),
                        }
                    }
                });
            if let Err(e) = spawned {
                log::warn!("single copy: {e}");
            }
        }
    }

    fn accept(stream: TcpStream, token: &str) -> io::Result<Vec<String>> {
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        let limit = ((MAX_LINKS + 3) * (MAX_LINE + 1)) as u64;
        let mut lines = BufReader::new(stream.try_clone()?.take(limit)).lines();
        let mut next = || -> io::Result<String> {
            lines
                .next()
                .unwrap_or_else(|| Err(io::Error::other("the message ended early")))
        };
        if next()? != HELLO {
            return Err(io::Error::other("not a hand-over"));
        }
        if next()? != token {
            return Err(io::Error::other("wrong token"));
        }
        let mut links = Vec::new();
        loop {
            let line = next()?;
            if line.is_empty() {
                break;
            }
            if links.len() < MAX_LINKS && line.len() <= MAX_LINE {
                links.push(line);
            }
        }
        (&stream).write_all(b"ok\n")?;
        Ok(links)
    }
}

// ---- The tray mark ----------------------------------------------------

/// What waits for you: mentions in channels and every unread message in
/// direct conversations and groups, leaving out what is muted (the web
/// client's app badge).
pub fn attention(w: &Workspace) -> i64 {
    w.conversations
        .iter()
        .filter(|c| sidebar_order::level(w, c) != Level::None)
        .map(|c| {
            if counts_every_message(c) {
                c.unread_count
            } else {
                c.mention_count
            }
        })
        .sum()
}

fn counts_every_message(c: &Conversation) -> bool {
    c.kind == ConversationKind::Direct || c.party.is_some()
}

/// Whether the tray shows the mark on this device (on unless switched off).
pub fn tray_mark_on(saved: &Saved) -> bool {
    saved.get("trayMark").is_none_or(|v| *v != 0.0)
}

/// Whether the tray mark is switched on (on until the session says otherwise).
pub fn tray_mark_shown(cx: &App) -> bool {
    cx.try_global::<Desktop>().is_none_or(|d| d.mark_on)
}

pub fn set_tray_mark(session: &Entity<Session>, on: bool, cx: &mut App) {
    let config = session.read(cx).config.clone();
    let mut saved: Saved = config.section(SECTION);
    saved.insert("trayMark".into(), if on { 1.0 } else { 0.0 });
    if let Err(e) = config.save_section(SECTION, &saved) {
        log::warn!("save the tray mark: {e}");
    }
    let desktop = cx.global_mut::<Desktop>();
    desktop.mark_on = on;
    desktop.marked = None;
    refresh_mark(session, cx);
}

/// The count the tray shows now: zero when signed out or switched off.
pub fn tray_count(session: &Session, cx: &App) -> i64 {
    if !tray_mark_shown(cx) {
        return 0;
    }
    session.client().map_or(0, |c| attention(&c.read()))
}

fn refresh_mark(session: &Entity<Session>, cx: &mut App) {
    let count = tray_count(session.read(cx), cx);
    let desktop = cx.default_global::<Desktop>();
    if desktop.marked == Some(count) {
        return;
    }
    desktop.marked = Some(count);
    let n = tray::icon_size();
    let (picture, tooltip) = if count > 0 {
        (
            with_mark(tray::pixels(n), n),
            t!("desktop-tray-waiting", count = count),
        )
    } else {
        (tray::pixels(n), t!("core-app-name").to_string())
    };
    tray::update(cx, picture, &tooltip);
}

/// The icon with a red dot in its top-right corner, cut out of the disc by
/// a transparent ring so it reads on any tray.
pub fn with_mark(mut rgba: Vec<u8>, n: usize) -> Vec<u8> {
    let r = n as f32 * 0.22;
    let ring = n as f32 * 0.07;
    let (cx, cy) = (n as f32 - r - 0.5, r + 0.5);
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let px = &mut rgba[(y * n + x) * 4..][..4];
            if d <= r {
                px.copy_from_slice(&[237, 66, 69, 255]);
            } else if d <= r + ring {
                px.copy_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    rgba
}

// ---- Window placement -------------------------------------------------

/// The smallest window the app opens (as `window_options`).
const MIN: (f32, f32) = (640.0, 480.0);

/// Where to open the main window, from where it was last; `None` for the
/// default (centred).
pub fn placement(session: &Session, cx: &App) -> Option<WindowBounds> {
    let screens: Vec<Bounds<Pixels>> = cx.displays().iter().map(|d| d.bounds()).collect();
    place(&session.config.section(SECTION), &screens)
}

/// The saved place, if its title bar is still on one of `screens`.
pub fn place(saved: &Saved, screens: &[Bounds<Pixels>]) -> Option<WindowBounds> {
    let get = |key: &str| saved.get(key).copied().filter(|v| v.is_finite());
    let (x, y) = (get("x")? as f32, get("y")? as f32);
    let width = (get("width")? as f32).max(MIN.0);
    let height = (get("height")? as f32).max(MIN.1);
    let visible = screens.iter().any(|s| {
        let (sx, sy) = (f32::from(s.origin.x), f32::from(s.origin.y));
        let (sw, sh) = (f32::from(s.size.width), f32::from(s.size.height));
        let across = (x + width).min(sx + sw) - x.max(sx);
        across >= 100.0 && y >= sy && y <= sy + sh - 40.0
    });
    if !visible {
        return None;
    }
    let bounds = Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    };
    Some(if get("maximized").is_some_and(|m| m != 0.0) {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    })
}

/// `saved` with the window's place written in.
pub fn with_place(mut saved: Saved, bounds: WindowBounds) -> Saved {
    let (rect, maximized) = match bounds {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
    };
    for (key, value) in [
        ("x", f32::from(rect.origin.x)),
        ("y", f32::from(rect.origin.y)),
        ("width", f32::from(rect.size.width)),
        ("height", f32::from(rect.size.height)),
        ("maximized", if maximized { 1.0 } else { 0.0 }),
    ] {
        saved.insert(key.into(), f64::from(value));
    }
    saved
}

/// Save the window's place once it has stopped moving for a moment.
fn remember(bounds: WindowBounds, cx: &mut App) {
    let rect = match bounds {
        WindowBounds::Windowed(b) | WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => b,
    };
    // A minimised window on Windows sits far off-screen; keep the last place.
    if f32::from(rect.origin.x) < -10_000.0 || f32::from(rect.size.width) < 1.0 {
        return;
    }
    let desktop = cx.default_global::<Desktop>();
    let scheduled = desktop.unsaved.is_some();
    desktop.unsaved = Some(bounds);
    if scheduled {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(400))
            .await;
        cx.update(|cx| {
            let Some(bounds) = cx.global_mut::<Desktop>().unsaved.take() else {
                return;
            };
            let Some(session) = session(cx) else { return };
            let config = session.read(cx).config.clone();
            let saved = with_place(config.section(SECTION), bounds);
            if let Err(e) = config.save_section(SECTION, &saved) {
                log::warn!("save the window place: {e}");
            }
        });
    })
    .detach();
}

// ---- The system's handler for irc:// ----------------------------------

/// Registering IRCord as the system's handler for `irc://` and `ircs://`.
/// Windows keeps it in the registry under the current user, Linux in the
/// desktop's MIME defaults (`xdg-mime`) with a desktop entry for IRCord;
/// macOS reads it from the app bundle, so there is nothing to register.
pub mod links {
    use std::io;
    use std::path::PathBuf;
    use std::process::Command;

    pub const SCHEMES: [&str; 2] = ["irc", "ircs"];

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Handler {
        /// IRCord opens them.
        Ours,
        /// Another app does, by its name when known.
        Other(String),
        /// Nothing is set.
        None,
        /// The system decides from the app bundle (macOS).
        Bundle,
        /// This desktop has no way to choose (no `xdg-mime`).
        Unavailable,
    }

    /// The desktop entry's file name (as `packaging/linux`).
    pub const DESKTOP_ID: &str = "com.ircord.gpui.desktop";

    /// Run a helper program without a console window flashing up on Windows.
    fn run(program: &str, args: &[&str]) -> io::Result<String> {
        let mut command = Command::new(program);
        command.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let output = command.output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(io::Error::other(format!(
                "{program}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    fn exe() -> io::Result<PathBuf> {
        std::env::current_exe()
    }

    /// Who opens `irc://` links now. Runs helper programs: call it off the
    /// UI thread.
    pub fn handler() -> io::Result<Handler> {
        if cfg!(target_os = "macos") {
            return Ok(Handler::Bundle);
        }
        if cfg!(windows) {
            return windows_handler();
        }
        match run("xdg-mime", &["query", "default", "x-scheme-handler/irc"]) {
            Ok(out) => Ok(linux_handler(&out)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Handler::Unavailable),
            Err(e) => Err(e),
        }
    }

    /// What `xdg-mime query default` printed, as a handler.
    pub fn linux_handler(out: &str) -> Handler {
        match out.trim() {
            "" => Handler::None,
            id if id == DESKTOP_ID => Handler::Ours,
            id => Handler::Other(id.trim_end_matches(".desktop").to_string()),
        }
    }

    fn windows_handler() -> io::Result<Handler> {
        // A choice made in Windows' settings wins over the class.
        let choice = run(
            "reg",
            &[
                "query",
                r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\irc\UserChoice",
                "/v",
                "ProgId",
            ],
        )
        .ok()
        .and_then(|out| reg_value(&out));
        if let Some(prog) = choice.filter(|p| !p.eq_ignore_ascii_case("irc")) {
            return Ok(Handler::Other(prog));
        }
        let Ok(out) = run(
            "reg",
            &[
                "query",
                r"HKCU\Software\Classes\irc\shell\open\command",
                "/ve",
            ],
        ) else {
            return Ok(Handler::None);
        };
        let exe = exe()?.to_string_lossy().to_lowercase();
        Ok(match reg_value(&out) {
            Some(command) if command.to_lowercase().contains(&exe) => Handler::Ours,
            Some(command) => Handler::Other(command),
            None => Handler::None,
        })
    }

    /// The data of the value `reg query` printed: the text after `REG_SZ`.
    pub fn reg_value(out: &str) -> Option<String> {
        out.lines()
            .find_map(|line| line.split_once("REG_SZ").map(|(_, v)| v.trim().to_string()))
            .filter(|v| !v.is_empty())
    }

    /// Make IRCord the handler. Runs helper programs: call it off the UI
    /// thread.
    pub fn register() -> io::Result<()> {
        if cfg!(target_os = "macos") {
            return Ok(());
        }
        let exe = exe()?;
        if cfg!(windows) {
            let exe = exe.to_string_lossy();
            for scheme in SCHEMES {
                let key = format!(r"HKCU\Software\Classes\{scheme}");
                let icon = format!("\"{exe}\",0");
                let command = format!("\"{exe}\" \"%1\"");
                run("reg", &["add", &key, "/ve", "/d", "URL:IRC link", "/f"])?;
                run("reg", &["add", &key, "/v", "URL Protocol", "/d", "", "/f"])?;
                let icon_key = format!(r"{key}\DefaultIcon");
                run("reg", &["add", &icon_key, "/ve", "/d", &icon, "/f"])?;
                let open_key = format!(r"{key}\shell\open\command");
                run("reg", &["add", &open_key, "/ve", "/d", &command, "/f"])?;
            }
            return Ok(());
        }
        if !installed_entry() {
            let dir = data_home()?.join("applications");
            std::fs::create_dir_all(&dir)?;
            let entry = desktop_entry(
                include_str!("../../../../packaging/linux/com.ircord.gpui.desktop"),
                &exe.to_string_lossy(),
            );
            std::fs::write(dir.join(DESKTOP_ID), entry)?;
        }
        let mut args = vec!["default", DESKTOP_ID];
        let types: Vec<String> = SCHEMES
            .iter()
            .map(|s| format!("x-scheme-handler/{s}"))
            .collect();
        args.extend(types.iter().map(String::as_str));
        run("xdg-mime", &args).map(|_| ())
    }

    /// `$XDG_DATA_HOME`, by default `~/.local/share`.
    fn data_home() -> io::Result<PathBuf> {
        if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
            return Ok(PathBuf::from(dir));
        }
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|home| PathBuf::from(home).join(".local/share"))
            .ok_or_else(|| io::Error::other("no home folder"))
    }

    /// Whether a desktop entry for IRCord is installed (by `install.sh` or a
    /// package) in any of the desktop's data folders.
    fn installed_entry() -> bool {
        let mut dirs: Vec<PathBuf> = data_home().into_iter().collect();
        let system = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        dirs.extend(system.split(':').map(PathBuf::from));
        dirs.iter()
            .any(|d| d.join("applications").join(DESKTOP_ID).is_file())
    }

    /// The packaged desktop entry, starting `exe`.
    pub fn desktop_entry(template: &str, exe: &str) -> String {
        let quoted = if exe.contains([' ', '"', '\\']) {
            format!("\"{}\"", exe.replace('\\', "\\\\").replace('"', "\\\""))
        } else {
            exe.to_string()
        };
        template
            .lines()
            .map(|line| match line.strip_prefix("Exec=") {
                Some(_) => format!("Exec={quoted} %u"),
                None => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::{Saved, instance, links, links_from_args, place, tray, with_mark, with_place};
    use gpui_kit::{Bounds, Pixels, WindowBounds, point, px, size};
    use std::time::Duration;

    #[test]
    fn arguments_keep_only_links() {
        let args = [
            "--flag",
            "irc://irc.libera.chat/#rust",
            "IRCS://Example.org:6697/chan",
            "https://chat.example/channel/c1?m=9",
            "notes.txt",
            "irc://bad\nline",
        ]
        .map(String::from);
        assert_eq!(
            links_from_args(args),
            [
                "irc://irc.libera.chat/#rust",
                "IRCS://Example.org:6697/chan",
                "https://chat.example/channel/c1?m=9"
            ]
        );
    }

    fn temp() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ircord-dk1-{}", ircord_client::client::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_second_copy_hands_its_links_to_the_first() {
        let dir = temp();
        let instance::Claim::First(Some(listener)) = instance::claim(&dir, &[]) else {
            panic!("the first copy listens");
        };
        let (send, receive) = std::sync::mpsc::channel();
        listener.serve(move |links| send.send(links).unwrap());
        let links = vec!["irc://irc.libera.chat/#rust".to_string()];
        assert!(matches!(
            instance::claim(&dir, &links),
            instance::Claim::Handed
        ));
        assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), links);
        // A copy with nothing to open still hands over: the window comes up.
        assert!(matches!(
            instance::claim(&dir, &[]),
            instance::Claim::Handed
        ));
        assert!(
            receive
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_stale_or_forged_instance_file_does_not_stop_the_app() {
        let dir = temp();
        // A port nobody listens on any more.
        let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = gone.local_addr().unwrap().port();
        drop(gone);
        std::fs::write(dir.join(instance::FILE), format!("{port} abc\n")).unwrap();
        let instance::Claim::First(Some(listener)) = instance::claim(&dir, &[]) else {
            panic!("a stale file is taken over");
        };
        let (send, receive) = std::sync::mpsc::channel();
        listener.serve(move |links| send.send(links).unwrap());
        // Someone who read the port but not the token gets nowhere.
        let text = std::fs::read_to_string(dir.join(instance::FILE)).unwrap();
        let port: u16 = text.split(' ').next().unwrap().parse().unwrap();
        std::fs::write(dir.join(instance::FILE), format!("{port} wrong\n")).unwrap();
        let claim = instance::claim(&dir, &["irc://x/#y".to_string()]);
        assert!(matches!(claim, instance::Claim::First(_)));
        assert!(receive.recv_timeout(Duration::from_millis(300)).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    fn screen() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1920.), px(1080.)),
        }
    }

    #[test]
    fn the_window_opens_where_it_was_if_that_is_on_a_screen() {
        let at = |x: f32, y: f32| {
            WindowBounds::Windowed(Bounds {
                origin: point(px(x), px(y)),
                size: size(px(1000.), px(700.)),
            })
        };
        let saved = with_place(Saved::new(), at(200., 100.));
        assert_eq!(place(&saved, &[screen()]), Some(at(200., 100.)));
        // On a screen that is no longer there.
        let away = with_place(Saved::new(), at(3000., 100.));
        assert_eq!(place(&away, &[screen()]), None);
        // Nothing saved yet.
        assert_eq!(place(&Saved::new(), &[screen()]), None);
        // Maximised, with the size to restore to.
        let bounds = Bounds {
            origin: point(px(10.), px(10.)),
            size: size(px(300.), px(200.)),
        };
        let max = with_place(Saved::new(), WindowBounds::Maximized(bounds));
        let restored = Bounds {
            origin: bounds.origin,
            size: size(px(640.), px(480.)),
        };
        assert_eq!(
            place(&max, &[screen()]),
            Some(WindowBounds::Maximized(restored))
        );
    }

    #[test]
    fn the_mark_is_a_red_dot_top_right() {
        let n = 32;
        let marked = with_mark(tray::pixels(n), n);
        let at = |x: usize, y: usize| marked[(y * n + x) * 4..][..4].to_vec();
        assert_eq!(at(n - 7, 7), [237, 66, 69, 255]);
        assert_eq!(at(4, 16), [88, 101, 242, 255], "the disc stays");
    }

    #[test]
    fn handlers_read_from_the_system() {
        assert_eq!(
            links::linux_handler("com.ircord.gpui.desktop\n"),
            links::Handler::Ours
        );
        assert_eq!(
            links::linux_handler("hexchat.desktop\n"),
            links::Handler::Other("hexchat".into())
        );
        assert_eq!(links::linux_handler(""), links::Handler::None);
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Classes\\irc\\shell\\open\\command\r\n    (Default)    REG_SZ    \"C:\\Apps\\ircord.exe\" \"%1\"\r\n";
        assert_eq!(
            links::reg_value(out).as_deref(),
            Some("\"C:\\Apps\\ircord.exe\" \"%1\"")
        );
        let entry = links::desktop_entry(
            "[Desktop Entry]\nExec=ircord %u\nName=IRCord",
            "/opt/my apps/ircord",
        );
        assert!(entry.contains("Exec=\"/opt/my apps/ircord\" %u\n"));
        assert!(entry.contains("Name=IRCord"));
    }
}
