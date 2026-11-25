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
use crate::ui::frame_motion;
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

/// The colour a person's profile card and picture frame take, as the web's
/// `ProfileDialog` tint: the accent they picked, else their name colour,
/// else `None` for the theme's accent.
pub fn tint(m: &Member, colored_names: bool, theme: &Theme) -> Option<Hsla> {
    people::accent_color(m)
        .or_else(|| name_color(m, colored_names, theme))
        .map(theme::hex)
}

/// `picture` (a disc `size` across) in the frame `frame`, drawn in `color`
/// as the web's `.avatar-frame-*` (`identity.css`): in a band 5 px wide
/// around the picture. The two gradient frames hollow their band with
/// `backdrop`; the others draw only their border. Without a frame only the
/// band's room shows.
pub fn framed(
    picture: impl IntoElement,
    frame: Option<people::AvatarFrame>,
    color: Hsla,
    backdrop: Hsla,
    size: Pixels,
    key: &str,
) -> Stateful<Div> {
    framed_with(picture, frame, color, backdrop, size, key, false)
}

/// [`framed`] on the profile card and its preview in Settings: the frames
/// move (`frame_motion`) while they are on screen, unless the theme asks for
/// reduced motion, which keeps them still.
pub fn framed_live(
    picture: impl IntoElement,
    frame: Option<people::AvatarFrame>,
    color: Hsla,
    backdrop: Hsla,
    size: Pixels,
    key: &str,
    theme: &Theme,
) -> Stateful<Div> {
    framed_with(
        picture,
        frame,
        color,
        backdrop,
        size,
        key,
        !theme.reduced_motion,
    )
}

fn framed_with(
    picture: impl IntoElement,
    frame: Option<people::AvatarFrame>,
    color: Hsla,
    backdrop: Hsla,
    size: Pixels,
    key: &str,
    live: bool,
) -> Stateful<Div> {
    use people::AvatarFrame;
    let kind = frame.map_or("none", AvatarFrame::key);
    let outer = div()
        .id(ElementId::Name(format!("avatar-frame-{key}-{kind}").into()))
        .size(size + px(10.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center();
    // The two gradients are a band of colour with the backdrop inside it;
    // a moving frame draws something over the band, under the picture.
    let band = |outer: Stateful<Div>,
                fill: Option<Background>,
                over: Option<AnyElement>,
                picture: AnyElement| {
        let outer = match fill {
            Some(fill) => outer.bg(fill),
            None => outer,
        };
        outer.children(over).child(
            div()
                .size(size + px(4.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(backdrop)
                .child(picture),
        )
    };
    let picture = picture.into_any_element();
    match frame {
        None => outer.child(picture),
        Some(AvatarFrame::Ring) => outer.border_3().border_color(color).child(picture),
        Some(AvatarFrame::Glow) => {
            let outer = outer.border_2().border_color(color);
            if live {
                // The glow's blur breathes, so it is drawn rather than styled.
                outer.child(frame_motion::glow(color)).child(picture)
            } else {
                outer
                    .shadow(vec![BoxShadow {
                        color: color.opacity(0.65),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(18.),
                        spread_radius: px(2.),
                        inset: false,
                    }])
                    .child(picture)
            }
        }
        // The web's conic sweep from their colour through amber, cyan and
        // violet. GPUI draws two-stop linear gradients, so a still frame runs
        // from their colour to cyan; a moving one is drawn in pieces, as the
        // sweep itself, turning.
        Some(AvatarFrame::Gradient) if live => band(
            outer,
            None,
            Some(frame_motion::spectrum(color).into_any_element()),
            picture,
        ),
        Some(AvatarFrame::Gradient) => band(
            outer,
            Some(linear_gradient(
                210.,
                linear_color_stop(color, 0.),
                linear_color_stop(theme::hex(0x22d3ee), 1.),
            )),
            None,
            picture,
        ),
        // Gold whatever their colour, as on the web; a glint crosses it now
        // and then when it moves.
        Some(AvatarFrame::Gold) => band(
            outer,
            Some(linear_gradient(
                135.,
                linear_color_stop(theme::hex(0xfde68a), 0.),
                linear_color_stop(theme::hex(0xb45309), 1.),
            )),
            live.then(|| frame_motion::glint().into_any_element()),
            picture,
        ),
        Some(AvatarFrame::Dashed) => outer
            .border_2()
            .border_dashed()
            .border_color(color)
            .child(picture),
    }
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
        .and_then(|s| people::member_picture(m.avatar_src.as_deref(), m.avatar_url.as_deref(), s))
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
    fn the_tint_is_their_accent_else_their_name_colour() {
        use super::tint;
        use crate::theme::{self, Theme};
        let theme = Theme::dark();
        let mut m = ircord_client::testing::member("m-bob", "bob");
        assert_eq!(tint(&m, false, &theme), None);
        assert!(tint(&m, true, &theme).is_some());
        m.name_color = Some("#ff8800".into());
        let named = tint(&m, false, &theme);
        assert!(named.is_some());
        m.profile_accent_color = Some("#22d3ee".into());
        assert_eq!(tint(&m, false, &theme), Some(theme::hex(0x22d3ee)));
        assert_ne!(tint(&m, false, &theme), named);
    }

    #[test]
    fn letters_are_sized_as_on_the_web() {
        assert_eq!(letter_size(px(TINY)), px(11.));
        assert_eq!(letter_size(px(SMALL)), px(12.));
        assert_eq!(letter_size(px(NORMAL)), px(15.));
        assert_eq!(letter_size(px(LARGE)), px(30.));
    }
}
