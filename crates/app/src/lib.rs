//! The IRCord GPUI app: GPUI views over `ircord-client`.
//!
//! How the crate is laid out for many hands at once:
//!
//! - Text goes through [`t!`] and the catalogs in `i18n/en/` ([`i18n`]).
//! - Colours come from the runtime [`theme`].
//! - Dialogs, menus, toasts and other controls come from [`ui`].
//! - A feature lives in its own module under [`features`] and adds itself
//!   to the views through [`registry`]; the main pane shows a [`route`].

#[macro_use]
pub mod i18n;

pub mod composer;
pub mod features;
pub mod images;
pub mod loading;
pub mod media;
pub mod members;
pub mod menu_order;
pub mod message;
pub mod notify;
pub mod registry;
pub mod route;
pub mod session;
pub mod settings;
pub mod sidebar;
pub mod signin;
pub mod theme;
pub mod timeline;
pub mod tray;
pub mod ui;
pub mod workspace;

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::config::ConfigDir;
use ircord_client::credentials::{CredentialStore, SystemStore};

use loading::LoadingView;
use session::{Phase, Session, SessionEvent};
use signin::SignInView;
use workspace::WorkspaceView;

const MAC_TITLEBAR_HEIGHT: f32 = 28.;

/// Set up everything the views draw from: the theme, the registry and the
/// features that fill it, and the default keys. Call once, after
/// `gpui_kit::init`.
pub fn init(cx: &mut App) {
    theme::init(cx);
    ui::focus::init(cx);
    cx.default_global::<registry::Registry>();
    settings::init(cx);
    features::init(cx);
    registry::bind_keys(cx, &|_| None);
}

/// The window's root: the sign-in screens, the loading screen or the
/// workspace.
pub struct MainView {
    session: Entity<Session>,
    screen: Screen,
    _subscriptions: Vec<Subscription>,
}

pub enum Screen {
    SignIn(Entity<SignInView>),
    Loading(Entity<LoadingView>),
    Workspace(Entity<WorkspaceView>),
}

#[derive(PartialEq)]
enum Kind {
    SignIn,
    Loading,
    Workspace,
}

impl Screen {
    fn kind(&self) -> Kind {
        match self {
            Screen::SignIn(_) => Kind::SignIn,
            Screen::Loading(_) => Kind::Loading,
            Screen::Workspace(_) => Kind::Workspace,
        }
    }
}

fn kind_for(session: &Session) -> Kind {
    match session.phase {
        Phase::SignedIn { .. } if loading::bootstrapped(session) => Kind::Workspace,
        Phase::SignedIn { .. } => Kind::Loading,
        _ => Kind::SignIn,
    }
}

impl MainView {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let screen = Self::screen_for(&session, window, cx);
        let subscriptions = vec![
            cx.subscribe_in(
                &session,
                window,
                |this, session, event: &SessionEvent, window, cx| {
                    let check = match event {
                        SessionEvent::Phase => true,
                        SessionEvent::Changed(changes) => changes.reset,
                        _ => false,
                    };
                    if check {
                        if kind_for(session.read(cx)) != this.screen.kind() {
                            this.screen = Self::screen_for(session, window, cx);
                        }
                        cx.notify();
                    }
                },
            ),
            cx.observe_window_activation(window, |this, window, cx| {
                let active = window.is_window_active();
                this.session.update(cx, |s, _| s.set_window_active(active));
            }),
        ];
        Self {
            session,
            screen,
            _subscriptions: subscriptions,
        }
    }

    fn screen_for(
        session: &Entity<Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Screen {
        match kind_for(session.read(cx)) {
            Kind::Workspace => {
                Screen::Workspace(cx.new(|cx| WorkspaceView::new(session.clone(), window, cx)))
            }
            Kind::Loading => Screen::Loading(cx.new(|cx| LoadingView::new(session.clone(), cx))),
            Kind::SignIn => {
                Screen::SignIn(cx.new(|cx| SignInView::new(session.clone(), window, cx)))
            }
        }
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx);
        let content = match &self.screen {
            Screen::SignIn(view) => view.clone().into_any_element(),
            Screen::Loading(view) => view.clone().into_any_element(),
            Screen::Workspace(view) => view.clone().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .text_color(theme.text)
            .font_family(theme.font.clone())
            .when(
                cfg!(target_os = "macos")
                    && !window.is_fullscreen()
                    && !matches!(self.screen, Screen::Workspace(_)),
                |d| {
                    d.child(
                        gpui_kit::component::TitleBar::new()
                            .h(px(MAC_TITLEBAR_HEIGHT))
                            .border_b_0()
                            .bg(theme.sidebar),
                    )
                },
            )
            .child(div().flex_1().min_h_0().bg(theme.main).child(content))
    }
}

/// The open main window, if any.
struct MainWindow(Option<WindowHandle<Root>>);
impl Global for MainWindow {}

use gpui_kit::base::Root;

pub fn window_options(session: &Session, cx: &App) -> WindowOptions {
    let width = session
        .state
        .window_width
        .unwrap_or(1200.)
        .clamp(640., 4000.);
    let height = session
        .state
        .window_height
        .unwrap_or(800.)
        .clamp(480., 3000.);
    // Where the window was last, if that is still on a screen (DK1).
    let bounds = features::desktop::placement(session, cx).unwrap_or_else(|| {
        WindowBounds::Windowed(Bounds::centered(None, size(px(width), px(height)), cx))
    });
    WindowOptions {
        window_bounds: Some(bounds),
        titlebar: Some(TitlebarOptions {
            title: Some(t!("core-app-name").into()),
            appears_transparent: cfg!(target_os = "macos"),
            traffic_light_position: cfg!(target_os = "macos").then(|| point(px(14.), px(8.))),
        }),
        app_owns_titlebar_drag: cfg!(target_os = "macos"),
        window_min_size: Some(size(px(640.), px(480.))),
        app_id: Some(ircord_client::config::APP_ID.into()),
        ..Default::default()
    }
}

/// Bring the main window forward, opening a new one if it was closed.
pub fn show_main_window(session: Entity<Session>, cx: &mut App) {
    if let Some(handle) = cx.try_global::<MainWindow>().and_then(|w| w.0)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    open_main_window(session, cx);
}

pub fn open_main_window(session: Entity<Session>, cx: &mut App) {
    let options = window_options(session.read(cx), cx);
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        let view = cx.new(|cx| MainView::new(session.clone(), window, cx));
        let session = session.clone();
        window.on_window_should_close(cx, move |window, cx| {
            let bounds = window.bounds();
            session.update(cx, |s, _| {
                s.save_window_size(f32::from(bounds.size.width), f32::from(bounds.size.height));
                s.set_window_active(false);
            });
            true
        });
        view
    });
    match opened {
        Ok((handle, _)) => {
            let handle = handle.downcast::<Root>();
            cx.set_global(MainWindow(handle));
        }
        Err(e) => log::error!("could not open the window: {e}"),
    }
}

/// Start the app.
pub fn run() {
    let runtime = match ircord_client::client::runtime() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("IRCord could not start its network runtime: {e}");
            std::process::exit(1);
        }
    };
    let config = match ConfigDir::platform() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("IRCord could not find its settings folder: {e}");
            std::process::exit(1);
        }
    };
    init_logging(&config);
    let credentials: Arc<dyn CredentialStore> = Arc::new(SystemStore);
    let use_tray = !cfg!(all(feature = "hot-reload", debug_assertions));
    let quit_mode = if use_tray && tray::keeps_running() {
        QuitMode::Explicit
    } else {
        QuitMode::Default
    };
    let app = gpui_kit::application()
        .with_assets(ui::icon::Assets)
        .with_quit_mode(quit_mode);
    // Links the system hands a running app (macOS), and a click on the Dock
    // icon with no window open (DK1).
    app.on_open_urls(features::desktop::open_urls);
    app.on_reopen(features::desktop::reopen);
    app.run(move |cx| {
        notify::identify(cx);
        gpui_kit::init(cx);
        init(cx);
        #[cfg(target_os = "macos")]
        install_app_menu(cx);
        images::ImageClient::install(runtime.clone(), cx);
        let session = cx.new(|cx| Session::new(runtime, config, credentials, cx));
        notify::install(session.clone(), cx);
        if use_tray && let Err(e) = tray::install(session.clone(), cx) {
            log::error!("no tray icon: {e}");
            cx.set_quit_mode(QuitMode::Default);
        }
        open_main_window(session, cx);
        cx.activate(true);
    });
}

#[cfg(target_os = "macos")]
gpui_kit::actions!(ircord, [Quit]);

#[cfg(target_os = "macos")]
fn install_app_menu(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus([
        Menu::new(t!("core-app-name")).items([MenuItem::action(t!("core-tray-quit"), Quit)])
    ]);
}

/// Log to stderr, and to `ircord.log` in the settings folder (a Windows
/// release build has no console). Panics are logged too.
fn init_logging(config: &ConfigDir) {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"));
    let file = std::fs::create_dir_all(config.path())
        .and_then(|()| std::fs::File::create(config.path().join("ircord.log")));
    if let Ok(file) = file {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }
    builder.init();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        default(info);
    }));
}
