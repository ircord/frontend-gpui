//! Live cases for package S1: a personal theme is saved, used and removed
//! through a real backend.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use ircord_client::ServerAddress;
use ircord_client::api::{Api, ApiError};
use ircord_client::client::{random_id, runtime};
use ircord_client::contract::{
    Appearance, AppearanceFont, SaveAppAppearanceRequest, StoredAppearance, Themes,
};
use ircord_client::store::appearance::{self as look, Look, Mode, Seed};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn saves_uses_and_removes_a_personal_theme() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpuis");
    let password = random_id();
    wait_for_backend(&rt, &address);
    let channel = format!("#{}", unique("look"));
    rt.block_on(setup_account(
        &address, &username, &password, &irc, &channel,
    ));
    let api = Api::new(address.clone());
    rt.block_on(api.login(&username, &password))
        .expect("sign in");

    // A new theme from the current look, with its own dark surface.
    let look = Look::default();
    let mut theme = look::new_theme(&look, "Live sea", "IRCord desktop test", "");
    look::set_seed(&mut theme, Mode::Dark, Seed::Surface, 0x102030);
    let id = theme.id.clone().unwrap();
    let library = rt
        .block_on(api.save_theme(&theme, None, false))
        .expect("save the theme");
    let stored = library
        .themes
        .iter()
        .find(|t| t.id.as_deref() == Some(id.as_str()))
        .expect("the theme is in the library")
        .clone();
    assert_eq!(stored.name.as_deref(), Some("Live sea"));
    assert_eq!(look::drawn_seeds(&stored, Mode::Dark).surface, 0x102030);
    let version = stored.updated_at.clone().expect("the server versions it");

    // Saving over a version that moved on is refused.
    let mut edited = stored.clone();
    look::set_seed(&mut edited, Mode::Dark, Seed::Accent, 0x40c0ff);
    let stale = rt
        .block_on(api.save_theme(&edited, Some("2000-01-01T00:00:00Z"), false))
        .unwrap_err();
    assert!(
        matches!(stale, ApiError::Http { status: 412, .. }),
        "{stale:?}"
    );
    let library = rt
        .block_on(api.save_theme(&edited, Some(&version), false))
        .expect("save over the loaded version");
    let saved = library
        .themes
        .iter()
        .find(|t| t.id.as_deref() == Some(id.as_str()))
        .unwrap();
    assert_eq!(look::drawn_seeds(saved, Mode::Dark).accent, 0x40c0ff);

    // Use it: the appearance stores the theme and a font, and draws it.
    let mut settings = Appearance::default();
    let mut chosen = Look::of(&settings);
    chosen.theme = Some(id.clone());
    chosen.font = AppearanceFont::Manrope;
    chosen.write(&mut settings);
    let stored_look: StoredAppearance = rt
        .block_on(api.call(
            ircord_client::api::Method::PUT,
            "/api/v1/app/appearance",
            &SaveAppAppearanceRequest { settings },
        ))
        .expect("save the appearance");
    let read_back = Look::of(stored_look.settings.as_ref().unwrap());
    assert_eq!(read_back.theme.as_deref(), Some(id.as_str()));
    assert_eq!(read_back.font, AppearanceFont::Manrope);
    let drawn = look::compile(&read_back, Mode::Dark, &library.themes, false);
    assert_eq!(drawn.tokens.main, 0x102030);

    // Exported and imported again, it is a second, separate theme.
    let copy = look::import_theme(&look::export_theme(saved), "test", "").unwrap();
    let library = rt
        .block_on(api.save_theme(&copy, None, false))
        .expect("save the import");
    assert_eq!(library.themes.len(), 2);

    // Remove both.
    rt.block_on(api.delete_theme(&id)).expect("delete");
    let library: Themes = rt
        .block_on(api.delete_theme(copy.id.as_deref().unwrap()))
        .expect("delete the import");
    assert!(library.themes.is_empty());
    let read: Themes = rt
        .block_on(api.get("/api/v1/app/themes"))
        .expect("read the library");
    assert!(read.themes.is_empty());
}
