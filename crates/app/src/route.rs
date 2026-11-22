//! What the main pane shows, and the back/forward history through it.
//!
//! A conversation is drawn by the workspace itself; every other route is
//! drawn by the pane a feature registers for it (`Registry::panes`), so a
//! package adds a page without editing the workspace. The right-hand panel
//! is separate: it is a panel ID the session holds, drawn by the panel a
//! feature registers (`Registry::panels`).

use gpui_kit::SharedString;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    Conversation(String),
    /// A network's server window: its raw log and connection details.
    ServerWindow(String),
    Search,
    Inbox,
    Saved,
    Drafts,
    /// Links shared in a conversation, or everywhere when `None`.
    Links(Option<String>),
    Settings,
    /// A page a feature defines beyond the ones above.
    Page(SharedString),
}

/// Which registered pane draws a route.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PaneKind {
    ServerWindow,
    Search,
    Inbox,
    Saved,
    Drafts,
    Links,
    Settings,
    Page(SharedString),
}

impl Route {
    /// The pane that draws this route; `None` for a conversation, which the
    /// workspace draws itself.
    pub fn pane(&self) -> Option<PaneKind> {
        Some(match self {
            Route::Conversation(_) => return None,
            Route::ServerWindow(_) => PaneKind::ServerWindow,
            Route::Search => PaneKind::Search,
            Route::Inbox => PaneKind::Inbox,
            Route::Saved => PaneKind::Saved,
            Route::Drafts => PaneKind::Drafts,
            Route::Links(_) => PaneKind::Links,
            Route::Settings => PaneKind::Settings,
            Route::Page(id) => PaneKind::Page(id.clone()),
        })
    }

    pub fn conversation(&self) -> Option<&str> {
        match self {
            Route::Conversation(id) => Some(id),
            _ => None,
        }
    }
}

/// Back and forward through the routes visited, like a browser.
#[derive(Clone, Debug, Default)]
pub struct History {
    current: Option<Route>,
    back: Vec<Route>,
    forward: Vec<Route>,
}

/// Routes kept behind the current one.
const DEPTH: usize = 50;

impl History {
    pub fn current(&self) -> Option<&Route> {
        self.current.as_ref()
    }

    /// The route visible behind a modal settings route.
    pub fn previous(&self) -> Option<&Route> {
        self.back.last()
    }

    /// Go to `route`. Returns false if it is already showing.
    pub fn push(&mut self, route: Route) -> bool {
        if self.current.as_ref() == Some(&route) {
            return false;
        }
        if let Some(previous) = self.current.replace(route) {
            self.back.push(previous);
            if self.back.len() > DEPTH {
                self.back.remove(0);
            }
        }
        self.forward.clear();
        true
    }

    /// Show `route` without adding a history entry (a selection fixed up
    /// after a reconnect, say).
    pub fn replace(&mut self, route: Route) {
        self.current = Some(route);
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn back(&mut self) -> Option<&Route> {
        let previous = self.back.pop()?;
        if let Some(current) = self.current.replace(previous) {
            self.forward.push(current);
        }
        self.current.as_ref()
    }

    pub fn forward(&mut self) -> Option<&Route> {
        let next = self.forward.pop()?;
        if let Some(current) = self.current.replace(next) {
            self.back.push(current);
        }
        self.current.as_ref()
    }

    /// Drop every entry `keep` rejects, such as conversations that no
    /// longer exist.
    pub fn retain(&mut self, keep: impl Fn(&Route) -> bool) {
        self.back.retain(&keep);
        self.forward.retain(&keep);
        if self.current.as_ref().is_some_and(|r| !keep(r)) {
            self.current = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(id: &str) -> Route {
        Route::Conversation(id.into())
    }

    #[test]
    fn back_and_forward() {
        let mut h = History::default();
        assert!(h.push(conv("a")));
        assert!(!h.push(conv("a")), "the same route twice is one entry");
        h.push(Route::Search);
        h.push(conv("b"));
        assert_eq!(h.back(), Some(&Route::Search));
        assert_eq!(h.back(), Some(&conv("a")));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), Some(&Route::Search));
        h.push(Route::Inbox);
        assert!(!h.can_go_forward(), "a new route clears forward");
        assert_eq!(h.current(), Some(&Route::Inbox));
    }

    #[test]
    fn retain_drops_gone_routes() {
        let mut h = History::default();
        h.push(conv("a"));
        h.push(conv("gone"));
        h.push(conv("b"));
        h.retain(|r| r != &conv("gone"));
        assert_eq!(h.back(), Some(&conv("a")));
    }

    #[test]
    fn panes() {
        assert_eq!(conv("a").pane(), None);
        assert_eq!(Route::Links(None).pane(), Some(PaneKind::Links));
        assert_eq!(conv("a").conversation(), Some("a"));
    }
}
