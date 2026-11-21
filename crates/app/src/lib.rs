//! The IRCord desktop app: GPUI views over `ircord-client`.

pub mod images;
pub mod members;
pub mod notify;
pub mod rows;
pub mod session;
pub mod sidebar;
pub mod signin;
pub mod theme;
pub mod timeline;
pub mod tray;
pub mod workspace;

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::config::ConfigDir;
use ircord_client::credentials::{CredentialStore, SystemStore};

use session::{Phase, Session, SessionEvent};
use signin::SignInView;
use workspace::WorkspaceView;

/// The window's root: the sign-in screens or the workspace.
pub struct MainView {
    session: Entity<Session>,
    screen: Screen,
    _subscriptions: Vec<Subscription>,
}

pub enum Screen {
    SignIn(Entity<SignInView>),
    Workspace(Entity<WorkspaceView>),
}

impl MainView {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let screen = Self::screen_for(&session, window, cx);
        let subscriptions = vec![
            cx.subscribe_in(
                &session,
                window,
                |this, session, event: &SessionEvent, window, cx| {
                    if let SessionEvent::Phase = event {
                        let signed_in = matches!(session.read(cx).phase, Phase::SignedIn { .. });
                        let showing_workspace = matches!(this.screen, Screen::Workspace(_));
                        if signed_in != showing_workspace {
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
        if matches!(session.read(cx).phase, Phase::SignedIn { .. }) {
            Screen::Workspace(cx.new(|cx| WorkspaceView::new(session.clone(), window, cx)))
        } else {
            Screen::SignIn(cx.new(|cx| SignInView::new(session.clone(), window, cx)))
        }
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }
}

impl Render for MainView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.screen {
            Screen::SignIn(view) => view.clone().into_any_element(),
            Screen::Workspace(view) => view.clone().into_any_element(),
        };
        div()
            .size_full()
            .bg(theme::c(theme::MAIN))
            .text_color(theme::c(theme::TEXT))
            .font_family(".SystemUIFont")
            .child(content)
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
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(width), px(height)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("IRCord".into()),
            ..Default::default()
        }),
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
    let quit_mode = if tray::keeps_running() {
        QuitMode::Explicit
    } else {
        QuitMode::Default
    };
    gpui_kit::application()
        .with_quit_mode(quit_mode)
        .run(move |cx| {
            notify::identify(cx);
            gpui_kit::init(cx);
            cx.set_http_client(Arc::new(images::ImageClient::new(runtime.clone())));
            let session = cx.new(|cx| Session::new(runtime, config, credentials, cx));
            notify::install(session.clone(), cx);
            if let Err(e) = tray::install(session.clone(), cx) {
                log::error!("no tray icon: {e}");
                cx.set_quit_mode(QuitMode::Default);
            }
            open_main_window(session, cx);
            cx.activate(true);
        });
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
