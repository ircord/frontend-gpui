//! The personal theme background settings shared with the web client.

use ircord_contract::{
    ThemeBackground, ThemeBackgroundFit as Fit, ThemeBackgroundPlacement as Placement,
    ThemeBackgroundPosition as Position,
};

use super::{Tokens, contrast, mix, parse_hex};

pub const MAX_IMAGE_BYTES: usize = 25 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Background {
    pub path: String,
    pub placement: Placement,
    pub fit: Fit,
    pub position: Position,
    pub dim: u8,
    pub blur: u8,
    pub panel_opacity: u8,
}

impl Background {
    pub(super) fn compile(image: &ThemeBackground, colors: &Tokens) -> Option<Self> {
        let path = image.url.as_ref().filter(|path| is_upload_path(path))?;
        let (dim_floor, panel_floor) = contrast_floors(image, colors);
        Some(Self {
            path: path.clone(),
            placement: match image.placement {
                Some(Placement::App) => Placement::App,
                _ => Placement::Conversation,
            },
            fit: match image.fit {
                Some(Fit::Contain) => Fit::Contain,
                Some(Fit::Tile) => Fit::Tile,
                _ => Fit::Cover,
            },
            position: image
                .position
                .filter(|p| *p != Position::Unknown)
                .unwrap_or(Position::Center),
            dim: bounded(image.dim, 55, 0, 90).max(dim_floor),
            blur: bounded(image.blur, 0, 0, 20),
            panel_opacity: bounded(image.panel_opacity, 85, 40, 100).max(panel_floor),
        })
    }
}

/// Minimum dimming and panel opacity that keep the theme's text readable.
pub fn contrast_floors(image: &ThemeBackground, colors: &Tokens) -> (u8, u8) {
    let tones = match image.tones.as_deref() {
        Some([dark, light]) => parse_hex(dark).zip(parse_hex(light)),
        _ => None,
    };
    let Some((dark, light)) = tones else {
        return (60, 85);
    };
    let readable_at = |cover, percent| {
        [dark, light].into_iter().all(|tone| {
            let surface = mix(tone, cover, f64::from(percent) / 100.);
            contrast(colors.text, surface) >= 4.5
                && contrast(colors.body, surface) >= 4.5
                && contrast(colors.muted, surface) >= 3.
        })
    };
    let floor = |cover, start: u8, end| {
        (start..=end)
            .step_by(5)
            .find(|p| readable_at(cover, *p))
            .unwrap_or(end)
    };
    (floor(colors.main, 0, 90), floor(colors.sidebar, 40, 100))
}

/// Store the instance path from an upload response, as Theme Studio does.
/// Any host in the response is discarded. Background readers use their own server.
pub fn instance_upload_path(address: &str, server: &url::Url) -> Option<String> {
    let url = server.join(address).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && is_upload_path(url.path()))
    .then(|| url.path().to_string())
}

fn bounded(value: Option<i64>, default: u8, min: u8, max: u8) -> u8 {
    value
        .unwrap_or(i64::from(default))
        .clamp(i64::from(min), i64::from(max)) as u8
}

/// Match the server's allowed background paths. The stored path is always
/// relative to the signed-in server, and never carries a query or fragment.
pub(crate) fn is_upload_path(path: &str) -> bool {
    let part = |text: &str, max, punctuation: &[u8]| {
        !text.is_empty()
            && text.len() <= max
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || punctuation.contains(&byte))
    };
    if let Some(id) = path.strip_prefix("/api/v1/uploads/") {
        return part(id, 80, b"-");
    }
    let Some(rest) = path.strip_prefix("/files/") else {
        return false;
    };
    let mut fields = rest.split('/');
    let (Some(id), Some(token), Some(name), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    part(id, 80, b"-_")
        && part(token, 200, b"-_")
        && part(name, 400, b"._~%$&+,:;=@-")
        && !matches!(
            name.to_ascii_lowercase().as_str(),
            "." | ".." | "%2e" | "%2e%2e" | ".%2e" | "%2e."
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::appearance::{Look, Mode, compile};

    #[test]
    fn backgrounds_use_the_web_defaults_and_respect_the_contrast_floors() {
        let mut tokens = compile(&Look::default(), Mode::Dark, &[], false).tokens;
        tokens.main = 0;
        tokens.sidebar = 0;
        tokens.text = 0xffffff;
        tokens.body = 0xffffff;
        tokens.muted = 0xffffff;
        let mut image = ThemeBackground {
            url: Some("/files/id/token/wallpaper.png".into()),
            blur: Some(25),
            ..Default::default()
        };
        let default = Background::compile(&image, &tokens).unwrap();
        assert_eq!(
            (default.dim, default.panel_opacity, default.blur),
            (60, 85, 20)
        );
        assert_eq!(default.placement, Placement::Conversation);
        assert_eq!(
            (default.fit, default.position),
            (Fit::Cover, Position::Center)
        );
        image.dim = Some(0);
        image.panel_opacity = Some(40);
        image.tones = Some(vec!["#000000".into(), "#ffffff".into()]);
        let measured = Background::compile(&image, &tokens).unwrap();
        assert_eq!((measured.dim, measured.panel_opacity), (55, 55));
        image.url = None;
        assert_eq!(Background::compile(&image, &tokens), None);
    }

    #[test]
    fn only_instance_upload_paths_can_be_loaded() {
        for path in ["/files/a/_b/a%20b.webp", "/api/v1/uploads/u-1"] {
            assert!(is_upload_path(path));
        }
        for path in [
            "//elsewhere.example/files/a/b/a.png",
            "https://elsewhere.example/files/a/b/a.png",
            "/files/a/b/%2e%2e",
            "/files/a/b/a.png?query",
            "/api/v1/uploads/a/another",
            "/api/v1/account",
        ] {
            assert!(!is_upload_path(path), "{path}");
        }
    }

    #[test]
    fn saved_upload_paths_discard_the_response_origin_but_keep_an_escaped_name() {
        let server = url::Url::parse("https://chat.example.org").unwrap();
        assert_eq!(
            instance_upload_path(
                "https://files.example.org/files/id/tok/my%20photo.png",
                &server
            )
            .as_deref(),
            Some("/files/id/tok/my%20photo.png")
        );
        assert_eq!(
            instance_upload_path("/files/id/tok/a.png?different", &server),
            None
        );
        assert_eq!(
            instance_upload_path("file:///files/id/tok/a.png", &server),
            None
        );
    }
}
