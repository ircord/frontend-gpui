//! Desktop integration (package DK1):
//!
//! - **One running copy.** The first copy holds a lock in the settings
//!   folder and listens on a loopback port named in `instance` there. A
//!   later copy hands its links to it over that port (with the token from
//!   the same file) and exits, saying the app already runs when it was
//!   started without links; the first copy brings its window up and opens
//!   them ([`hand_over`], [`instance`]). macOS never starts a second copy
//!   from Finder: it hands links to the running app ([`open_urls`]).
//! - **Links.** `irc://` and `ircs://` links, and links to a message on the
//!   signed-in server, open in the app ([`open_link`]); links that arrive
//!   before the workspace has loaded wait for it. Registering IRCord as the
//!   system's handler for `irc://` is [`links`], shown in Settings → This
//!   device.
//! - **Tray mark.** The tray icon carries a red dot while mentions or direct
//!   messages wait, as the web client's app badge counts them ([`attention`]).
//! - **Window placement.** The main window opens where it was last, maximised
//!   if it was, as long as that place is still on a screen ([`placement`]).
//! - **The AppImage's menu entry.** Started from an AppImage with no menu
//!   entry for it, the app offers to add one for the current user, starting
//!   the AppImage where it is; Not now is remembered for that path. An entry
//!   the app wrote for an AppImage at a path that is gone follows it here
//!   ([`check_menu_entry`]). Updates replace the file in place, so the entry
//!   stays right.
//!
//! What this device keeps lives in the `desktop` section of the device state.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::config::ConfigDir;
use ircord_client::contract::{Conversation, ConversationKind};
use ircord_client::format::links::{network_name_from_host, parse_irc_url, parse_message_link};
use ircord_client::store::Workspace;
use ircord_client::store::sidebar_order::{self, Level};

use crate::features::{network_form, text};
use crate::registry::{self, BannerCtx, Slot};
use crate::session::{Phase, Session, SessionEvent};
use crate::theme;
use crate::ui::icon::IconName;
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
    cx.default_global::<MenuOffer>();
    registry::register(cx, |r| {
        r.banners.add(
            Slot::new("appimage-menu", |_: &BannerCtx, _, cx| {
                menu_banner(cx).map(IntoElement::into_any_element)
            })
            .order(30),
        );
    });
    cx.observe_new::<Session>(|session: &mut Session, _, cx| {
        #[cfg(target_os = "linux")]
        if let (Some(appimage), Ok(data)) = (
            ircord_platform::appimage::path(),
            ircord_platform::appimage::data_home(),
        ) {
            let session = cx.entity();
            cx.defer(move |cx| check_menu_entry(&session, appimage, data, cx));
        }
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
/// copy and return `false`. A second copy started without links tells the
/// person the app already runs; one started by a link just opens it there.
pub fn hand_over() -> bool {
    let links = links_from_args(std::env::args().skip(1));
    let claim = match ConfigDir::platform() {
        Ok(config) => {
            ircord_platform::desktop::allow_foreground();
            instance::claim(config.path(), &links, instance::PATIENCE)
        }
        Err(_) => instance::Claim::First(instance::Running::default()),
    };
    match claim {
        instance::Claim::Handed | instance::Claim::Unreachable => {
            if tells_the_person(&links) {
                ircord_platform::desktop::message(
                    t!("core-app-name"),
                    t!("desktop-already-running"),
                );
            }
            true
        }
        instance::Claim::First(running) => {
            // Held until the process ends, so the lock goes with it.
            running.serve(deliver);
            if !links.is_empty() {
                deliver(links);
            }
            false
        }
    }
}

/// Whether a second copy started with `links` says the app already runs: a
/// copy started from a link (a click in a browser) opens it without a word.
pub fn tells_the_person(links: &[String]) -> bool {
    links.is_empty()
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

/// The single running copy. The first copy holds a lock on `instance.lock`
/// in the settings folder for as long as it runs (the system lets go of it
/// when the process ends, even in a crash), listens on a loopback port and
/// names that port in `instance`. A later copy that cannot take the lock
/// hands its links over that port, with the token from the same file, and
/// exits even when the running copy does not answer: two copies never run
/// side by side. Without a lock (a file system that has none) the port
/// alone decides, as before.
pub mod instance {
    use std::fs::{File, TryLockError};
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// The file in the settings folder: `<port> <token>`.
    pub const FILE: &str = "instance";
    /// The file the running copy holds a lock on.
    pub const LOCK: &str = "instance.lock";
    const HELLO: &str = "ircord-handoff 1";
    pub const MAX_LINKS: usize = 32;
    pub const MAX_LINE: usize = 4096;
    const WAIT: Duration = Duration::from_secs(2);
    /// How long a second copy keeps trying a running copy that has the lock
    /// but is not listening yet (it is starting at the same moment).
    pub const PATIENCE: Duration = Duration::from_secs(5);

    pub enum Claim {
        /// Another copy took the links; this one should exit.
        Handed,
        /// Another copy runs but did not answer; this one should exit too.
        Unreachable,
        /// This is the running copy now.
        First(Running),
    }

    /// What the running copy keeps: the lock, and the listener when there
    /// is one.
    #[derive(Default)]
    pub struct Running {
        lock: Option<File>,
        listener: Option<Listener>,
    }

    pub struct Listener {
        socket: TcpListener,
        token: String,
    }

    impl Running {
        pub fn listening(&self) -> bool {
            self.listener.is_some()
        }

        pub fn locked(&self) -> bool {
            self.lock.is_some()
        }
    }

    /// Hand `links` to the copy running for `dir`, or become that copy.
    /// `patience` is how long to keep trying a copy that holds the lock.
    pub fn claim(dir: &Path, links: &[String], patience: Duration) -> Claim {
        let lock = match lock(dir) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => return hand_to_running(dir, links, patience),
            Err(e) => {
                log::warn!("single copy: no lock ({e}), going by the port alone");
                if handed(dir, links) {
                    return Claim::Handed;
                }
                None
            }
        };
        let listener = listen(dir)
            .inspect_err(|e| log::warn!("single copy: {e}"))
            .ok();
        Claim::First(Running { lock, listener })
    }

    /// The lock, `None` when another copy holds it.
    fn lock(dir: &Path) -> io::Result<Option<File>> {
        std::fs::create_dir_all(dir)?;
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(LOCK))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }

    /// Another copy holds the lock: keep trying its port, which it may not
    /// have written yet if both started together.
    fn hand_to_running(dir: &Path, links: &[String], patience: Duration) -> Claim {
        let until = Instant::now() + patience;
        loop {
            if handed(dir, links) {
                return Claim::Handed;
            }
            if Instant::now() >= until {
                log::warn!("single copy: the running copy did not answer");
                return Claim::Unreachable;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn handed(dir: &Path, links: &[String]) -> bool {
        read(dir).is_some_and(|(port, token)| send(port, &token, links).is_ok())
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

    impl Running {
        /// Take hand-overs on a thread of their own, calling `deliver` with
        /// each one's links (none: just show the window). The lock stays
        /// held until the process ends.
        pub fn serve(self, deliver: impl Fn(Vec<String>) + Send + 'static) {
            let Running { lock, listener } = self;
            // The lock lives as long as the process: the system drops it at
            // exit, whatever way the process ends.
            std::mem::forget(lock);
            let Some(listener) = listener else {
                return;
            };
            let spawned = std::thread::Builder::new()
                .name("ircord-handoff".into())
                .spawn(move || {
                    for stream in listener.socket.incoming().flatten() {
                        match accept(stream, &listener.token) {
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

// ---- The AppImage's menu entry ----------------------------------------

/// The device state section for the AppImage's menu entry: `menuDeclined`,
/// the AppImage for which Not now was pressed.
pub const APPIMAGE_SECTION: &str = "appimage";

/// The offer to add the AppImage to the applications menu.
#[derive(Default)]
struct MenuOffer {
    /// The AppImage offered, with the data home its entry goes under; `None`
    /// when nothing is offered.
    offered: Option<(PathBuf, PathBuf)>,
    session: Option<WeakEntity<Session>>,
    busy: bool,
    error: Option<String>,
}
impl Global for MenuOffer {}

/// Looks at the menu entry for the AppImage at `appimage` under `data` (a
/// data home) when a session starts: offers to add one when there is none for
/// it and Not now was not pressed for this path; moves one the app wrote for
/// this AppImage at another path that is gone. Linux only; [`init`] calls it with the
/// running AppImage, tests with their own.
pub fn check_menu_entry(session: &Entity<Session>, appimage: PathBuf, data: PathBuf, cx: &mut App) {
    #[cfg(target_os = "linux")]
    {
        use ircord_platform::appimage::{MenuEntry, menu_entry};
        match menu_entry(&data, &appimage) {
            MenuEntry::Current => {}
            MenuEntry::Moved => {
                cx.background_spawn(async move {
                    if let Err(e) = ircord_platform::appimage::add_menu_entry(&data, &appimage) {
                        log::warn!("move the menu entry to {}: {e}", appimage.display());
                    }
                })
                .detach();
            }
            MenuEntry::Missing | MenuEntry::Other => {
                let saved: BTreeMap<String, String> =
                    session.read(cx).config.section(APPIMAGE_SECTION);
                if saved.get("menuDeclined").map(Path::new) == Some(appimage.as_path()) {
                    return;
                }
                let offer = cx.global_mut::<MenuOffer>();
                offer.offered = Some((appimage, data));
                offer.session = Some(session.downgrade());
                cx.refresh_windows();
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (session, appimage, data, cx);
}

/// Add the offered menu entry, off the interface thread.
fn add_menu_entry(cx: &mut App) {
    let offer = cx.global_mut::<MenuOffer>();
    let Some((appimage, data)) = offer.offered.clone() else {
        return;
    };
    if offer.busy {
        return;
    }
    offer.busy = true;
    offer.error = None;
    cx.refresh_windows();
    #[cfg(target_os = "linux")]
    let task = cx.background_spawn(async move {
        ircord_platform::appimage::add_menu_entry(&data, &appimage).map_err(|e| e.to_string())
    });
    #[cfg(not(target_os = "linux"))]
    let task = {
        let _ = (appimage, data);
        cx.background_spawn(async { Err::<(), String>("not an AppImage".into()) })
    };
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            let offer = cx.global_mut::<MenuOffer>();
            offer.busy = false;
            match result {
                Ok(()) => offer.offered = None,
                Err(e) => {
                    log::warn!("add the menu entry: {e}");
                    offer.error = Some(e);
                }
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// The AppImage's menu entry was written elsewhere (making IRCord the
/// handler for `irc://` links does it): the offer goes.
pub fn menu_entry_written(cx: &mut App) {
    if let Some(offer) = cx.try_global::<MenuOffer>()
        && offer.offered.is_some()
        && !offer.busy
    {
        cx.global_mut::<MenuOffer>().offered = None;
        cx.refresh_windows();
    }
}

/// Not now: no offer for this AppImage again.
fn decline_menu_entry(cx: &mut App) {
    let offer = cx.global_mut::<MenuOffer>();
    let Some((appimage, _)) = offer.offered.take() else {
        return;
    };
    let session = offer.session.as_ref().and_then(WeakEntity::upgrade);
    cx.refresh_windows();
    let Some(session) = session else { return };
    let config = session.read(cx).config.clone();
    let mut saved: BTreeMap<String, String> = config.section(APPIMAGE_SECTION);
    saved.insert(
        "menuDeclined".into(),
        appimage.to_string_lossy().into_owned(),
    );
    if let Err(e) = config.save_section(APPIMAGE_SECTION, &saved) {
        log::warn!("remember the menu entry was declined: {e}");
    }
}

/// The banner offering the menu entry, while it is offered.
fn menu_banner(cx: &mut App) -> Option<impl IntoElement + use<>> {
    let offer = cx.try_global::<MenuOffer>()?;
    let (appimage, _) = offer.offered.as_ref()?;
    let text = t!(
        "desktop-appimage-offer",
        path = appimage.to_string_lossy().into_owned()
    )
    .to_string();
    let (busy, error) = (offer.busy, offer.error.clone());
    let theme = theme::get(cx).clone();
    Some(
        div()
            .id("appimage-menu-banner")
            .test_support()
            .role(Role::Status)
            .aria_label(SharedString::from(t!("desktop-appimage-label")))
            .aria_description(SharedString::from(text.clone()))
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_1p5()
            .flex_none()
            .text_sm()
            .text_color(theme.muted)
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.border)
            .child(ui::icon::icon(IconName::Info, theme.muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("appimage-menu-text")
                            .test_support()
                            .role(Role::Group)
                            .aria_label(SharedString::from(text.clone()))
                            .text_color(theme.text)
                            .child(text),
                    )
                    .when_some(error, |d, error| {
                        let error = t!("desktop-appimage-failed", error = error).to_string();
                        d.child(
                            div()
                                .id("appimage-menu-error")
                                .test_support()
                                .role(Role::Alert)
                                .aria_label(SharedString::from(error.clone()))
                                .text_color(theme.danger)
                                .child(error),
                        )
                    }),
            )
            .child(
                Button::new("appimage-menu-add")
                    .ghost()
                    .label(t!("desktop-appimage-add"))
                    .disabled(busy)
                    .on_click(|_, _, cx| add_menu_entry(cx)),
            )
            .child(
                Button::new("appimage-menu-decline")
                    .ghost()
                    .label(t!("desktop-appimage-decline"))
                    .disabled(busy)
                    .on_click(|_, _, cx| decline_menu_entry(cx)),
            ),
    )
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

    /// The AppImage this copy runs from, if it does (Linux).
    fn appimage() -> Option<PathBuf> {
        #[cfg(target_os = "linux")]
        return ircord_platform::appimage::path();
        #[allow(unreachable_code)]
        None
    }

    /// The program a link starts: the AppImage when this copy runs from
    /// one (its own path inside is gone once it quits), else this program.
    fn exe() -> io::Result<PathBuf> {
        appimage().map_or_else(std::env::current_exe, Ok)
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
        // An AppImage's entry starts the AppImage, wherever other entries are.
        if appimage().is_some() {
            #[cfg(target_os = "linux")]
            ircord_platform::appimage::add_menu_entry(&data_home()?, &exe)?;
        } else if !installed_entry() {
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

    const SOON: Duration = Duration::from_millis(300);

    /// Far longer than any test machine stalls. A claim that treats a dead
    /// copy as running waits out its patience and ends in `Unreachable`, which
    /// `first_within` rejects; one that waits and then still starts cannot
    /// finish inside it either.
    const FOREVER: Duration = Duration::from_secs(60);

    fn first(dir: &std::path::Path) -> instance::Running {
        first_within(dir, SOON)
    }

    fn first_within(dir: &std::path::Path, patience: Duration) -> instance::Running {
        let instance::Claim::First(running) = instance::claim(dir, &[], patience) else {
            panic!("the first copy runs");
        };
        assert!(running.locked() && running.listening());
        running
    }

    #[test]
    fn a_second_copy_hands_its_links_to_the_first() {
        let dir = temp();
        let (send, receive) = std::sync::mpsc::channel();
        first(&dir).serve(move |links| send.send(links).unwrap());
        let links = vec!["irc://irc.libera.chat/#rust".to_string()];
        assert!(matches!(
            instance::claim(&dir, &links, SOON),
            instance::Claim::Handed
        ));
        assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), links);
        // A copy with nothing to open still hands over: the window comes up,
        // and that copy says the app already runs.
        assert!(matches!(
            instance::claim(&dir, &[], SOON),
            instance::Claim::Handed
        ));
        assert!(
            receive
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .is_empty()
        );
        assert!(super::tells_the_person(&[]));
        assert!(!super::tells_the_person(&links));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_running_copy_that_does_not_answer_still_stops_a_second() {
        let dir = temp();
        // The running copy holds the lock, but its port is gone (or it has
        // not written it yet, or a firewall eats the connection).
        let running = first(&dir);
        std::fs::write(dir.join(instance::FILE), "1 forged\n").unwrap();
        let started = std::time::Instant::now();
        assert!(matches!(
            instance::claim(&dir, &["irc://x/#y".to_string()], SOON),
            instance::Claim::Unreachable
        ));
        assert!(started.elapsed() >= SOON, "it waits for a copy starting up");
        // A wrong token is refused, and the second copy still does not run.
        drop(running);
        let (send, receive) = std::sync::mpsc::channel();
        first(&dir).serve(move |links| send.send(links).unwrap());
        let text = std::fs::read_to_string(dir.join(instance::FILE)).unwrap();
        let port: u16 = text.split(' ').next().unwrap().parse().unwrap();
        std::fs::write(dir.join(instance::FILE), format!("{port} wrong\n")).unwrap();
        assert!(matches!(
            instance::claim(&dir, &["irc://x/#y".to_string()], SOON),
            instance::Claim::Unreachable
        ));
        assert!(receive.recv_timeout(SOON).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_copy_starting_late_is_waited_for() {
        let dir = temp();
        // The lock is taken, the port follows a moment later (two copies
        // started together).
        let lock = std::fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(instance::LOCK))
            .unwrap();
        lock.lock().unwrap();
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let late = dir.clone();
        let server = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            std::fs::write(late.join(instance::FILE), format!("{port} t\n")).unwrap();
            let (stream, _) = socket.accept().unwrap();
            let mut lines = std::io::BufRead::lines(std::io::BufReader::new(&stream));
            let mut got = Vec::new();
            while let Some(Ok(line)) = lines.next() {
                if line.is_empty() {
                    break;
                }
                got.push(line);
            }
            std::io::Write::write_all(&mut &stream, b"ok\n").unwrap();
            got
        });
        assert!(matches!(
            instance::claim(&dir, &["irc://x/#y".to_string()], Duration::from_secs(5)),
            instance::Claim::Handed
        ));
        assert_eq!(
            server.join().unwrap(),
            ["ircord-handoff 1", "t", "irc://x/#y"]
        );
        drop(lock);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_stale_instance_file_does_not_stop_the_app() {
        let dir = temp();
        // A copy that crashed: its file names a port nobody listens on, and
        // its lock went with the process.
        // The port is still held here, but by the test: nothing may contact
        // it, which `accept` below sees without reading a clock.
        let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        gone.set_nonblocking(true).unwrap();
        let port = gone.local_addr().unwrap().port();
        std::fs::write(dir.join(instance::FILE), format!("{port} abc\n")).unwrap();
        std::fs::write(dir.join(instance::LOCK), "").unwrap();
        // The claim must neither wait on the dead copy (a wait as long as its
        // patience fails the bound) nor try its port (a delay of any length,
        // such as a connect that times out on Windows, shows as a connection).
        let started = std::time::Instant::now();
        let running = first_within(&dir, FOREVER);
        assert!(started.elapsed() < FOREVER, "no waiting on a dead copy");
        assert_eq!(
            gone.accept().map(|_| ()).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "the dead copy's port is not tried"
        );
        drop(gone);
        let text = std::fs::read_to_string(dir.join(instance::FILE)).unwrap();
        assert!(!text.ends_with(" abc\n"), "the file names the new copy");
        // And when that copy ends in turn, the next one starts.
        drop(running);
        first(&dir);
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
