//! A person's picture, as the web client's `UserAvatar.vue`: the picture
//! they set when it is on this IRCord server (`people::picture_url`, the
//! web's #278 rule), else their initial on their tone. It can carry a
//! presence dot and a bot mark. The sizes are the web's: 22, 28, 36, 80.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{Member, MemberRole, MemberStatus};
use ircord_client::store::people;

use crate::theme::{self, Theme};
use crate::ui::icon::IconName;

pub const TINY: f32 = 22.;
pub const SMALL: f32 = 28.;
pub const NORMAL: f32 = 36.;
pub const LARGE: f32 = 80.;

/// A disc `size` across with the first letter of `name`: on `color` when
/// given, else on the web's default (lavender) tone.
pub fn avatar(name: &str, color: Option<Hsla>, size: Pixels, theme: &Theme) -> Div {
    let (fill, ink) = match color {
        Some(color) => (color, theme.on_accent),
        None => {
            let (fill, ink) = people::tone("");
            (theme::hex(fill), theme::hex(ink))
        }
    };
    initial(name, fill, ink, size)
}

fn initial(name: &str, fill: Hsla, ink: Hsla, size: Pixels) -> Div {
    let letter = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    div()
        .size(size)
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(fill)
        .text_color(ink)
        .text_size(letter_size(size))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(relative(1.))
        .child(letter)
}

/// The web's letter sizes: 11, 12, 15 and 30 px for its four discs, 16 in
/// the 40 px message gutter, and in proportion for the larger cards.
fn letter_size(size: Pixels) -> Pixels {
    let s = f32::from(size);
    px(if s <= TINY {
        11.
    } else if s <= SMALL {
        12.
    } else if s <= NORMAL {
        15.
    } else if s < 48. {
        16.
    } else {
        (s * 0.375).round()
    })
}

/// A person's name colour on the message background, as the web's
/// `useNameStyle`: the colour they chose always, the hashed one when
/// coloured names are on, else `None` for the plain text colour.
pub fn name_color(m: &Member, colored_names: bool, theme: &Theme) -> Option<u32> {
    if !colored_names && people::chosen_name_color(m).is_none() {
        return None;
    }
    Some(people::name_color(m, theme.dark, rgb(theme.main)))
}

/// `0xrrggbb` for a colour.
fn rgb(color: Hsla) -> u32 {
    let c = Rgba::from(color);
    [c.r, c.g, c.b].iter().fold(0, |acc, v| {
        (acc << 8) | (v.clamp(0., 1.) * 255.).round() as u32
    })
}

/// What to draw for one person.
pub struct Person<'a> {
    pub member: &'a Member,
    /// The name whose initial shows without a picture.
    pub name: &'a str,
    /// The server signed in to: pictures load only from there.
    pub server: Option<&'a url::Url>,
    /// Draw the presence dot on the corner.
    pub presence: bool,
    /// Draws the picture's test ID, `avatar-picture-{key}`.
    pub key: &'a str,
}

/// The picture `size` across. The disc is decorative (the web hides it from
/// screen readers): the row it sits in names the person.
pub fn person(p: Person, size: Pixels, theme: &Theme) -> Div {
    let m = p.member;
    let (fill, ink) = people::tone(&m.color);
    let (fill, ink) = (theme::hex(fill), theme::hex(ink));
    let fallback = {
        let name = p.name.to_string();
        move || initial(&name, fill, ink, size).into_any_element()
    };
    let face = match p
        .server
        .and_then(|s| people::picture_url(m.avatar_url.as_deref(), s))
    {
        Some(url) => div()
            .id(ElementId::Name(format!("avatar-picture-{}", p.key).into()))
            .test_support()
            .size(size)
            .rounded_full()
            .overflow_hidden()
            .child(
                img(crate::images::still(url.to_string().into()))
                    .size(size)
                    .rounded_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(fallback),
            )
            .into_any_element(),
        None => fallback(),
    };
    let edge = theme.sidebar;
    let large = f32::from(size) >= LARGE;
    let mut disc = div().relative().size(size).flex_none().child(face);
    if m.role == MemberRole::Bot {
        let mark = if large {
            28.
        } else if f32::from(size) <= TINY {
            14.
        } else {
            19.
        };
        let offset = if large { 0. } else { -5. };
        disc = disc.child(
            div()
                .absolute()
                .top(px(offset))
                .right(px(offset))
                .size(px(mark))
                .rounded_full()
                .border_2()
                .border_color(edge)
                .bg(theme.accent)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    gpui_kit::component::Icon::new(IconName::Bot)
                        .size(px(mark * 0.6))
                        .text_color(theme.on_accent),
                ),
        );
    }
    if p.presence {
        let dot = match m.status {
            MemberStatus::Online => theme.online,
            MemberStatus::Away => theme.away,
            _ => theme.offline,
        };
        let d = if large { 18. } else { 11. };
        disc = disc.child(
            div()
                .absolute()
                .bottom(px(-2.))
                .right(px(-2.))
                .size(px(d))
                .rounded_full()
                .border_2()
                .border_color(edge)
                .bg(dot),
        );
    }
    disc
}

#[cfg(test)]
mod tests {
    use super::{LARGE, NORMAL, SMALL, TINY, letter_size};
    use gpui_kit::px;

    #[test]
    fn letters_are_sized_as_on_the_web() {
        assert_eq!(letter_size(px(TINY)), px(11.));
        assert_eq!(letter_size(px(SMALL)), px(12.));
        assert_eq!(letter_size(px(NORMAL)), px(15.));
        assert_eq!(letter_size(px(LARGE)), px(30.));
    }
}
