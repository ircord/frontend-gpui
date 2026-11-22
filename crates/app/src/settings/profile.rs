//! Settings → Profile (package S4), as the web client's `ProfileSettings.vue`,
//! `ProfileEditor.vue` and `ProfileDetailsEditor.vue`: per server, your
//! picture (your own, a default one, or none), display name, name colour and
//! about line, then the richer details (status, headline, pronouns, where and
//! when you are, languages and interests, and how your card looks), saved
//! together. With no server yet, the account's own profile is edited.
//!
//! The picture helpers here are shared with the sign-up and the first-run
//! setup (`features/onboarding.rs`).

use crate::ui::focus::Reachable;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Colorize;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::ApiError;
use ircord_client::api::auth::{self, DefaultAvatar};
use ircord_client::api::uploads::{self, FileData, Progress};
use ircord_client::contract::{Member, ProfileUpdate};
use ircord_client::store::appearance::parse_hex;
use ircord_client::store::people;

use super::{block, frame, row};
use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new("profile", t!("profile-title"), |ctx, window, cx| {
                let view = view(ctx, window, cx);
                view.update(cx, |v, _| v.ctx = Some(ctx.clone()));
                view.into_any_element()
            })
            .group(SettingsGroup::User)
            .order(10)
            .keywords(
                t!("profile-keywords")
                    .split(',')
                    .map(|w| w.trim().to_string()),
            )
            .row(
                SettingsRow::new("profile-picture", t!("profile-picture"))
                    .keywords(t!("profile-picture-keywords")),
            )
            .row(
                SettingsRow::new("profile-defaults", t!("profile-default-pictures"))
                    .keywords(t!("profile-defaults-keywords")),
            )
            .row(
                SettingsRow::new("profile-name", t!("profile-display-name"))
                    .keywords(t!("profile-display-name-keywords")),
            )
            .row(SettingsRow::new("profile-color", t!("profile-name-color")))
            .row(SettingsRow::new("profile-bio", t!("profile-about-you")))
            .row(
                SettingsRow::new("profile-status", t!("profile-status"))
                    .keywords(t!("profile-status-keywords")),
            )
            .row(
                SettingsRow::new("profile-about", t!("profile-details"))
                    .description(t!("profile-details-help"))
                    .keywords(t!("profile-details-keywords")),
            )
            .row(SettingsRow::new(
                "profile-interests",
                t!("profile-languages-and-interests"),
            ))
            .row(
                SettingsRow::new("profile-look", t!("profile-look"))
                    .keywords(t!("profile-look-keywords")),
            ),
        );
    });
}

// ---- Pictures, shared with the sign-up and the first-run setup ----

/// Pictures are saved this many pixels across.
pub const AVATAR_SIDE: u32 = 256;
/// The largest data URL a profile takes.
const MAX_DATA_URL: usize = 180_000;
/// The largest picture file read (the web client's limit).
pub const MAX_AVATAR_FILE: usize = 5 << 20;

/// A picture to save, made from a picked file.
#[derive(Clone)]
pub struct Prepared {
    pub data_url: String,
    /// The same picture, ready to draw as a preview.
    pub preview: Arc<RenderImage>,
}

impl PartialEq for Prepared {
    fn eq(&self, other: &Self) -> bool {
        self.data_url == other.data_url
    }
}

/// Turn a picked image into what a profile takes: cropped to a square from
/// its middle, [`AVATAR_SIDE`] across, as a PNG data URL, or a JPEG one when
/// the PNG would be too large.
pub fn prepare_avatar(bytes: &[u8]) -> Result<Prepared, String> {
    if bytes.len() > MAX_AVATAR_FILE {
        return Err(t!("profile-image-too-large").into());
    }
    let image = image::load_from_memory(bytes).map_err(|_| t!("profile-image-open-failed"))?;
    let side = image.width().min(image.height());
    if side == 0 {
        return Err(t!("profile-image-open-failed").into());
    }
    let square = image.crop_imm(
        (image.width() - side) / 2,
        (image.height() - side) / 2,
        side,
        side,
    );
    let small = square.resize_exact(
        AVATAR_SIDE.min(side),
        AVATAR_SIDE.min(side),
        image::imageops::FilterType::Lanczos3,
    );
    let rgba = small.to_rgba8();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba.clone())
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|_| t!("profile-image-open-failed"))?;
    let mut data_url = format!("data:image/png;base64,{}", base64(&png));
    if data_url.len() > MAX_DATA_URL {
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(rgba.clone()).to_rgb8())
            .write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .map_err(|_| t!("profile-image-open-failed"))?;
        data_url = format!("data:image/jpeg;base64,{}", base64(&jpeg));
    }
    if data_url.len() > MAX_DATA_URL {
        return Err(t!("profile-image-too-large").into());
    }
    // GPUI draws BGRA.
    let mut bgra = rgba;
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Prepared {
        data_url,
        preview: Arc::new(RenderImage::new(vec![image::Frame::new(bgra)])),
    })
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// What happens to the picture on the next save.
#[derive(Clone, PartialEq, Default)]
pub enum AvatarChoice {
    /// Leave it as saved.
    #[default]
    Keep,
    Default(DefaultAvatar),
    Upload(Prepared),
    Remove,
}

impl AvatarChoice {
    /// Add the picture to `update`; `origin` is the server's, which a
    /// default picture's saved URL is on.
    pub fn patch(&self, origin: &str, update: &mut ProfileUpdate) {
        match self {
            AvatarChoice::Keep => {}
            AvatarChoice::Default(avatar) => {
                update.avatar_url =
                    Some(format!("{}{}", origin.trim_end_matches('/'), avatar.path()))
            }
            AvatarChoice::Upload(p) => update.avatar_data_url = Some(p.data_url.clone()),
            AvatarChoice::Remove => update.avatar_url = Some(String::new()),
        }
    }
}

/// A picture to draw: from the server, or picked and not saved yet.
#[derive(Clone)]
pub enum Picture {
    Url(SharedString),
    Image(Arc<RenderImage>),
    /// None: the initial of `name` shows.
    None,
}

/// What the picture will look like after a save with `choice`.
pub fn preview(choice: &AvatarChoice, saved: Option<&str>, session: &Session) -> Picture {
    let url = match choice {
        AvatarChoice::Upload(p) => return Picture::Image(p.preview.clone()),
        AvatarChoice::Remove => return Picture::None,
        AvatarChoice::Default(a) => a.path(),
        AvatarChoice::Keep => saved.unwrap_or_default().to_string(),
    };
    match resolve(&url, session) {
        Some(url) => Picture::Url(url.into()),
        None => Picture::None,
    }
}

/// A saved picture's address as something to load.
fn resolve(url: &str, session: &Session) -> Option<String> {
    if url.starts_with("https://") || url.starts_with("http://") {
        Some(url.to_string())
    } else if url.starts_with('/') {
        session.server().map(|s| s.join(url).to_string())
    } else {
        None
    }
}

/// A round picture `size` across, or the initial of `name`.
pub fn picture(p: &Picture, name: &str, size: Pixels, theme: &Theme) -> AnyElement {
    match p {
        Picture::Url(url) => img(crate::images::still(url.clone()))
            .size(size)
            .flex_none()
            .rounded_full()
            .into_any_element(),
        Picture::Image(image) => img(ImageSource::Render(image.clone()))
            .size(size)
            .flex_none()
            .rounded_full()
            .into_any_element(),
        Picture::None => ui::avatar::avatar(name, None, size, theme).into_any_element(),
    }
}

/// Ask for an image file and hand its bytes, prepared, to `done` (or the
/// reason it could not be used).
pub fn pick_picture(
    window: &mut Window,
    cx: &mut App,
    done: impl FnOnce(Result<Prepared, String>, &mut Window, &mut App) + 'static,
) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(t!("profile-upload-image").into()),
    });
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let prepared = cx
                .background_spawn(async move {
                    std::fs::read(&path)
                        .map_err(|_| t!("profile-image-open-failed").to_string())
                        .and_then(|bytes| prepare_avatar(&bytes))
                })
                .await;
            cx.update(|window, cx| done(prepared, window, cx)).ok();
        })
        .detach();
}

/// The default pictures of one server, read once.
#[derive(Default)]
struct Defaults {
    origin: String,
    avatars: Option<Rc<Vec<DefaultAvatar>>>,
}
impl Global for Defaults {}

/// The default pictures the server offers; empty until read, and when it
/// offers none.
pub fn default_avatars(session: &Entity<Session>, cx: &mut App) -> Rc<Vec<DefaultAvatar>> {
    let Some(server) = session.read(cx).server().cloned() else {
        return Rc::default();
    };
    let origin = server.origin();
    let defaults = cx.default_global::<Defaults>();
    if defaults.origin == origin {
        return defaults.avatars.clone().unwrap_or_default();
    }
    defaults.origin = origin.clone();
    defaults.avatars = None;
    let api = ircord_client::api::Api::new(server);
    let task = session
        .read(cx)
        .runtime
        .spawn(async move { api.default_avatars().await });
    cx.spawn(async move |cx| {
        let list = task.await.ok().and_then(Result::ok).unwrap_or_default();
        cx.update(|cx| {
            let defaults = cx.default_global::<Defaults>();
            if defaults.origin == origin {
                defaults.avatars = Some(Rc::new(list));
            }
            cx.refresh_windows();
        });
    })
    .detach();
    Rc::default()
}

/// Which default picture is picked and which is suggested, and whether
/// picking is off for now.
#[derive(Clone, Copy, Default)]
pub struct Marks<'a> {
    pub selected: Option<&'a str>,
    pub suggested: Option<&'a str>,
    pub disabled: bool,
}

/// The grid of default pictures, each `{id}-{avatar id}`.
pub fn default_picker(
    id: &str,
    avatars: &[DefaultAvatar],
    marks: Marks,
    session: &Session,
    theme: &Theme,
    on_pick: impl Fn(&DefaultAvatar, &mut Window, &mut App) + 'static,
) -> AnyElement {
    picker(id, avatars, marks, session, theme, true, on_pick)
}

/// The default pictures, in a short scroller when `capped` (onboarding) or
/// all of them at once (the Profile page, as on the web).
fn picker(
    id: &str,
    avatars: &[DefaultAvatar],
    marks: Marks,
    session: &Session,
    theme: &Theme,
    capped: bool,
    on_pick: impl Fn(&DefaultAvatar, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let Marks {
        selected,
        suggested,
        disabled,
    } = marks;
    let on_pick = Rc::new(on_pick);
    let mut grid = div()
        .id(ElementId::Name(id.to_string().into()))
        .role(Role::RadioGroup)
        .aria_label(t!("profile-default-pictures"))
        .flex()
        .flex_wrap()
        .gap_1()
        .when(capped, |d| d.max_h(px(112.)).overflow_y_scroll());
    for (i, avatar) in avatars.iter().enumerate() {
        let picked = selected == Some(avatar.id.as_str());
        let hint = suggested == Some(avatar.id.as_str());
        let url = resolve(&avatar.path(), session).unwrap_or_default();
        let on_pick = on_pick.clone();
        let chosen = avatar.clone();
        let label = if hint {
            t!("profile-default-picture-suggested", number = i + 1)
        } else {
            t!("profile-default-picture", number = i + 1)
        };
        grid = grid.child(
            div()
                .id(ElementId::Name(format!("{id}-{}", avatar.id).into()))
                .test_support()
                .role(Role::RadioButton)
                .reachable(theme)
                .aria_label(SharedString::from(label))
                .aria_selected(picked)
                .p_0p5()
                .rounded_full()
                .border_2()
                .border_color(if picked {
                    theme.accent
                } else if hint {
                    theme.muted
                } else {
                    theme.sidebar
                })
                .when(!disabled, |d| {
                    d.cursor_pointer()
                        .on_click(move |_, window, cx| on_pick(&chosen, window, cx))
                })
                .child(img(url).size(px(36.)).rounded_full()),
        );
    }
    grid.into_any_element()
}

/// The name a person chose for themselves, blank when the network's nick or
/// real name shows (the web client's `explicitDisplayName`).
pub fn explicit_name(m: &Member) -> String {
    use ircord_client::contract::MemberNameSource as S;
    if m.display_name.is_empty() {
        return String::new();
    }
    match m.name_source {
        Some(S::Profile) => m.display_name.clone(),
        Some(_) => String::new(),
        None if m.display_name == "You" || m.display_name == m.nick => String::new(),
        None => m.display_name.clone(),
    }
}

/// Text from an error the server sent back.
pub fn error_text(e: &ApiError) -> String {
    match e {
        ApiError::Http { detail, .. } => detail.clone(),
        e => e.to_string(),
    }
}

// ---- The section ----

struct ProfileGlobal(Entity<ProfileView>);
impl Global for ProfileGlobal {}

fn view(ctx: &SettingsCtx, window: &mut Window, cx: &mut App) -> Entity<ProfileView> {
    if let Some(ProfileGlobal(view)) = cx.try_global::<ProfileGlobal>()
        && view.read(cx).session == ctx.session
    {
        return view.clone();
    }
    let view = cx.new(|cx| ProfileView::new(ctx.session.clone(), window, cx));
    cx.set_global(ProfileGlobal(view.clone()));
    view
}

/// When a status clears.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClearAfter {
    Keep,
    Never,
    Minutes30,
    Hour1,
    Hours4,
    Today,
    Week,
}

impl ClearAfter {
    const ALL: [ClearAfter; 6] = [
        ClearAfter::Never,
        ClearAfter::Minutes30,
        ClearAfter::Hour1,
        ClearAfter::Hours4,
        ClearAfter::Today,
        ClearAfter::Week,
    ];

    fn key(self) -> &'static str {
        match self {
            ClearAfter::Keep => "keep",
            ClearAfter::Never => "never",
            ClearAfter::Minutes30 => "30m",
            ClearAfter::Hour1 => "1h",
            ClearAfter::Hours4 => "4h",
            ClearAfter::Today => "today",
            ClearAfter::Week => "week",
        }
    }

    fn label(self) -> SharedString {
        match self {
            ClearAfter::Keep => t!("profile-clear-keep").into(),
            ClearAfter::Never => t!("profile-clear-never").into(),
            ClearAfter::Minutes30 => t!("profile-clear-30m").into(),
            ClearAfter::Hour1 => t!("profile-clear-1h").into(),
            ClearAfter::Hours4 => t!("profile-clear-4h").into(),
            ClearAfter::Today => t!("profile-clear-today").into(),
            ClearAfter::Week => t!("profile-clear-week").into(),
        }
    }

    /// The deadline to send: `None` to leave it, `Some("")` for none.
    pub fn deadline(self, now: chrono::DateTime<chrono::Local>) -> Option<String> {
        use chrono::{Duration, TimeZone, Timelike};
        let at = match self {
            ClearAfter::Keep => return None,
            ClearAfter::Never => return Some(String::new()),
            ClearAfter::Minutes30 => now + Duration::minutes(30),
            ClearAfter::Hour1 => now + Duration::hours(1),
            ClearAfter::Hours4 => now + Duration::hours(4),
            ClearAfter::Today => {
                let end = now
                    .with_hour(23)
                    .and_then(|t| t.with_minute(59))
                    .and_then(|t| t.with_second(59))
                    .and_then(|t| t.with_nanosecond(0))
                    .unwrap_or(now);
                chrono::Local
                    .from_local_datetime(&end.naive_local())
                    .single()
                    .unwrap_or(end)
            }
            ClearAfter::Week => now + Duration::days(7),
        };
        Some(
            at.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
    }
}

/// The avatar frames the web client draws.
pub const FRAMES: [&str; 6] = ["none", "ring", "glow", "gradient", "gold", "dashed"];

fn frame_label(frame: &str) -> SharedString {
    match frame {
        "ring" => t!("profile-frame-ring").into(),
        "glow" => t!("profile-frame-glow").into(),
        "gradient" => t!("profile-frame-gradient").into(),
        "gold" => t!("profile-frame-gold").into(),
        "dashed" => t!("profile-frame-dashed").into(),
        _ => t!("profile-frame-none").into(),
    }
}

/// The text fields, in the order they are drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Name,
    Bio,
    Color,
    StatusEmoji,
    StatusText,
    Headline,
    Pronouns,
    Location,
    Timezone,
    Homepage,
    List(ProfileList),
    Accent,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ProfileList {
    Languages,
    Interests,
}

impl Field {
    const ALL: [Field; 13] = [
        Field::Name,
        Field::Bio,
        Field::Color,
        Field::StatusEmoji,
        Field::StatusText,
        Field::Headline,
        Field::Pronouns,
        Field::Location,
        Field::Timezone,
        Field::Homepage,
        Field::List(ProfileList::Languages),
        Field::List(ProfileList::Interests),
        Field::Accent,
    ];

    fn id(self) -> &'static str {
        match self {
            Field::Name => "profile-name-input",
            Field::Bio => "profile-bio-input",
            Field::Color => "profile-color-input",
            Field::StatusEmoji => "profile-status-emoji",
            Field::StatusText => "profile-status-text",
            Field::Headline => "profile-headline",
            Field::Pronouns => "profile-pronouns",
            Field::Location => "profile-location",
            Field::Timezone => "profile-timezone",
            Field::Homepage => "profile-homepage",
            Field::List(ProfileList::Languages) => "profile-languages",
            Field::List(ProfileList::Interests) => "profile-interests-input",
            Field::Accent => "profile-accent-input",
        }
    }

    fn label(self) -> SharedString {
        match self {
            Field::Name => t!("profile-display-name").into(),
            Field::Bio => t!("profile-about-you").into(),
            Field::Color => t!("profile-custom-color").into(),
            Field::StatusEmoji => t!("profile-status-emoji").into(),
            Field::StatusText => t!("profile-status-text").into(),
            Field::Headline => t!("profile-headline").into(),
            Field::Pronouns => t!("profile-pronouns").into(),
            Field::Location => t!("profile-location").into(),
            Field::Timezone => t!("profile-timezone").into(),
            Field::Homepage => t!("profile-homepage").into(),
            Field::List(ProfileList::Languages) => t!("profile-languages").into(),
            Field::List(ProfileList::Interests) => t!("profile-interests").into(),
            Field::Accent => t!("profile-accent").into(),
        }
    }

    fn placeholder(self) -> SharedString {
        match self {
            Field::Name => SharedString::default(),
            Field::Bio => t!("profile-bio-placeholder").into(),
            Field::Color | Field::Accent => "#3366cc".into(),
            Field::StatusEmoji => t!("profile-status-emoji-placeholder").into(),
            Field::StatusText => t!("profile-status-placeholder").into(),
            Field::Headline => t!("profile-headline-placeholder").into(),
            Field::Pronouns => t!("profile-pronouns-placeholder").into(),
            Field::Location => t!("profile-location-placeholder").into(),
            Field::Timezone => t!("profile-timezone-placeholder").into(),
            Field::Homepage => t!("profile-homepage-placeholder").into(),
            Field::List(ProfileList::Languages) => t!("profile-languages-placeholder").into(),
            Field::List(ProfileList::Interests) => t!("profile-interests-placeholder").into(),
        }
    }

    /// What `m` has saved in this field, as typed.
    fn saved(self, m: &Member) -> String {
        let text = |v: &Option<String>| v.clone().unwrap_or_default();
        match self {
            Field::Name => explicit_name(m),
            Field::Bio => m.bio.clone(),
            Field::Color => text(&m.name_color),
            Field::StatusEmoji => text(&m.status_emoji),
            Field::StatusText => text(&m.status_text),
            Field::Headline => text(&m.headline),
            Field::Pronouns => text(&m.pronouns),
            Field::Location => text(&m.location),
            Field::Timezone => text(&m.timezone),
            Field::Homepage => text(&m.homepage_url),
            Field::List(_) => String::new(),
            Field::Accent => text(&m.profile_accent_color),
        }
    }
}

/// Whether `text` is a `#RRGGBB` colour.
pub fn is_color(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Why the typed profile cannot be saved, if it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Problem {
    Color,
    Accent,
    Homepage,
    TooMany(ProfileList),
    Duplicate(ProfileList),
    LanguageTag,
    LongInterest,
}

impl Problem {
    fn text(self) -> SharedString {
        match self {
            Problem::Color | Problem::Accent => t!("profile-problem-color").into(),
            Problem::Homepage => t!("profile-problem-homepage").into(),
            Problem::TooMany(_) => t!("profile-problem-too-many").into(),
            Problem::Duplicate(_) => t!("profile-problem-duplicate").into(),
            Problem::LanguageTag => t!("profile-problem-language-tag").into(),
            Problem::LongInterest => t!("profile-problem-long-interest").into(),
        }
    }

    fn field(self) -> Field {
        match self {
            Problem::Color => Field::Color,
            Problem::Accent => Field::Accent,
            Problem::Homepage => Field::Homepage,
            Problem::TooMany(list) | Problem::Duplicate(list) => Field::List(list),
            Problem::LanguageTag => Field::List(ProfileList::Languages),
            Problem::LongInterest => Field::List(ProfileList::Interests),
        }
    }
}

fn validated_tag(list: ProfileList, text: &str, tags: &[String]) -> Result<String, Problem> {
    let tag = match list {
        ProfileList::Languages => people::language_tag(text).ok_or(Problem::LanguageTag)?,
        ProfileList::Interests if text.len() <= 32 && !text.chars().any(char::is_control) => {
            text.to_string()
        }
        ProfileList::Interests => return Err(Problem::LongInterest),
    };
    let key = tag.to_lowercase();
    if tags.iter().any(|item| item.to_lowercase() == key) {
        return Err(Problem::Duplicate(list));
    }
    if tags.len() >= 10 {
        return Err(Problem::TooMany(list));
    }
    Ok(tag)
}

pub struct ProfileView {
    session: Entity<Session>,
    ctx: Option<SettingsCtx>,
    /// The server whose profile shows; `None` edits the account's own.
    network: Option<String>,
    /// What is saved, once read.
    saved: Option<Member>,
    loading: bool,
    load_error: Option<String>,
    inputs: Vec<(Field, Entity<InputState>)>,
    languages: Vec<String>,
    interests: Vec<String>,
    name_color: Entity<ColorPickerState>,
    custom_color: bool,
    custom_accent: bool,
    avatar: AvatarChoice,
    banner: Option<String>,
    background: Option<String>,
    frame: String,
    clear_after: ClearAfter,
    uploading: Option<&'static str>,
    saving: bool,
    error: Option<SharedString>,
    problem: Option<Problem>,
    done: bool,
    _subscriptions: Vec<Subscription>,
}

impl ProfileView {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let mut inputs = Vec::new();
        let name_color = cx.new(|cx| ColorPickerState::new(window, cx));
        subscriptions.push(cx.subscribe_in(
            &name_color,
            window,
            |this: &mut Self, _, event: &ColorPickerEvent, window, cx| {
                if let ColorPickerEvent::Change(Some(value)) = event {
                    let color = Hsla { a: 1., ..*value };
                    if value.a < 1. {
                        this.name_color
                            .update(cx, |picker, cx| picker.set_value(color, window, cx));
                    }
                    let hex = color.to_hex();
                    this.input(Field::Color)
                        .update(cx, |input, cx| input.set_value(hex, window, cx));
                    this.done = false;
                    this.problem = None;
                    cx.notify();
                }
            },
        ));
        for field in Field::ALL {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(field.placeholder()));
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        if field == Field::Color
                            && let Some(color) = parse_hex(input.read(cx).value().trim())
                        {
                            this.name_color.update(cx, |picker, cx| {
                                picker.set_value(theme::hex(color), window, cx);
                            });
                        }
                        this.done = false;
                        this.problem = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => match field {
                        Field::List(list) => this.add_tag(list, window, cx),
                        _ => this.save(window, cx),
                    },
                    InputEvent::Focus | InputEvent::Blur if matches!(field, Field::List(_)) => {
                        cx.notify()
                    }
                    _ => {}
                },
            ));
            inputs.push((field, input));
        }
        // Your profile changed (saved here or on another device): show it,
        // unless you are in the middle of editing.
        subscriptions.push(cx.subscribe_in(
            &session,
            window,
            |this: &mut Self, _, event: &SessionEvent, window, cx| match event {
                SessionEvent::Changed(changes) if changes.members || changes.reset => {
                    if !this.dirty(cx) && !this.saving && !this.has_pending_tag(cx) {
                        this.reload(window, cx);
                    }
                }
                SessionEvent::Phase => this.pick_network(None, window, cx),
                _ => {}
            },
        ));
        let mut view = Self {
            session,
            ctx: None,
            network: None,
            saved: None,
            loading: false,
            load_error: None,
            inputs,
            languages: Vec::new(),
            interests: Vec::new(),
            name_color,
            custom_color: false,
            custom_accent: false,
            avatar: AvatarChoice::Keep,
            banner: None,
            background: None,
            frame: String::new(),
            clear_after: ClearAfter::Keep,
            uploading: None,
            saving: false,
            error: None,
            problem: None,
            done: false,
            _subscriptions: subscriptions,
        };
        view.pick_network(None, window, cx);
        view
    }

    fn input(&self, field: Field) -> &Entity<InputState> {
        &self.inputs.iter().find(|(f, _)| *f == field).unwrap().1
    }

    fn text(&self, field: Field, cx: &App) -> String {
        self.input(field).read(cx).value().to_string()
    }

    fn has_pending_tag(&self, cx: &App) -> bool {
        [ProfileList::Languages, ProfileList::Interests]
            .iter()
            .any(|list| {
                !self
                    .input(Field::List(*list))
                    .read(cx)
                    .value()
                    .trim()
                    .is_empty()
            })
    }

    fn tags_mut(&mut self, list: ProfileList) -> &mut Vec<String> {
        match list {
            ProfileList::Languages => &mut self.languages,
            ProfileList::Interests => &mut self.interests,
        }
    }

    fn add_tag(&mut self, list: ProfileList, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.saved.is_none() {
            return;
        }
        let field = Field::List(list);
        let text = self.text(field, cx);
        let raw = text.trim();
        if raw.is_empty() {
            return;
        }
        let tags = self.tags_mut(list);
        match validated_tag(list, raw, tags) {
            Ok(tag) => {
                tags.push(tag);
                self.input(field)
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.problem = None;
                self.done = false;
            }
            Err(problem) => self.problem = Some(problem),
        }
        cx.notify();
    }

    fn fallback_name(&self, member: &Member, cx: &App) -> SharedString {
        let client = self.session.read(cx).client();
        let workspace = client.map(|client| client.read());
        let network = workspace
            .as_ref()
            .and_then(|w| self.network.as_deref().and_then(|id| w.network(id)));
        match people::fallback_self_name(network, &member.nick) {
            Some(name) => SharedString::from(name.to_string()),
            None => t!("profile-self-no-name").into(),
        }
    }

    /// Show the profile on `network`, or on the selected (else first) one.
    fn pick_network(
        &mut self,
        network: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (chosen, known) = {
            let session = self.session.read(cx);
            let Some(client) = session.client() else {
                return;
            };
            let w = client.read();
            let valid = |id: &String| w.network(id).is_some();
            let chosen = network
                .filter(valid)
                .or_else(|| session.network.clone().filter(valid))
                .or_else(|| w.networks.first().map(|n| n.id.clone()));
            let known = chosen
                .as_ref()
                .and_then(|id| w.network(id))
                .and_then(|n| n.profile.clone())
                .or_else(|| chosen.is_none().then(|| w.user.clone()));
            (chosen, known)
        };
        self.network = chosen.clone();
        self.load_error = None;
        self.error = None;
        self.done = false;
        match known {
            Some(member) => self.fill(member, window, cx),
            None => {
                let Some(network) = chosen else {
                    return;
                };
                self.saved = None;
                self.loading = true;
                let Some(client) = self.session.read(cx).client().cloned() else {
                    return;
                };
                let id = network.clone();
                let task = client
                    .handle()
                    .request(move |api| async move { api.network_profile(&id).await });
                cx.spawn_in(window, async move |this, cx| {
                    let result = task.await.map_err(|e| e.to_string());
                    this.update_in(cx, |this, window, cx| {
                        if this.network.as_deref() != Some(network.as_str()) {
                            return;
                        }
                        this.loading = false;
                        match result {
                            Ok(Ok(member)) => {
                                client.change(|w| {
                                    auth::profile_saved(w, Some(&network), member.clone())
                                });
                                this.fill(member, window, cx);
                            }
                            Ok(Err(e)) => this.load_error = Some(error_text(&e)),
                            Err(e) => this.load_error = Some(e),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Read the saved profile again from the workspace.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let member = {
            let session = self.session.read(cx);
            let Some(client) = session.client() else {
                return;
            };
            let w = client.read();
            match &self.network {
                Some(id) => w.network(id).and_then(|n| n.profile.clone()),
                None => Some(w.user.clone()),
            }
        };
        if let Some(member) = member
            && Some(&member) != self.saved.as_ref()
        {
            self.fill(member, window, cx);
        }
    }

    /// Put `member`'s saved profile in the form.
    fn fill(&mut self, member: Member, window: &mut Window, cx: &mut Context<Self>) {
        let fallback = self.fallback_name(&member, cx);
        self.input(Field::Name)
            .update(cx, |input, cx| input.set_placeholder(fallback, window, cx));
        for (field, input) in &self.inputs {
            let value = field.saved(&member);
            input.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        let theme = theme::get(cx);
        let color = member
            .name_color
            .as_deref()
            .and_then(parse_hex)
            .or_else(|| ui::avatar::name_color(&member, true, theme))
            .map(theme::hex)
            .unwrap_or(theme.text);
        self.name_color
            .update(cx, |picker, cx| picker.set_value(color, window, cx));
        self.custom_color = member.name_color.as_deref().is_some_and(|c| !c.is_empty());
        self.custom_accent = member
            .profile_accent_color
            .as_deref()
            .is_some_and(|c| !c.is_empty());
        self.avatar = AvatarChoice::Keep;
        self.banner = member.banner_url.clone().filter(|u| !u.is_empty());
        self.background = member
            .profile_background_url
            .clone()
            .filter(|u| !u.is_empty());
        self.frame = member.avatar_frame.clone().unwrap_or_default();
        self.clear_after = ClearAfter::Keep;
        self.languages = member.languages.clone().unwrap_or_default();
        self.interests = member.interests.clone().unwrap_or_default();
        self.problem = None;
        self.saved = Some(member);
        cx.notify();
    }

    /// The typed changes, as the request that saves them, or what is wrong.
    fn update(&self, cx: &App) -> Result<ProfileUpdate, Problem> {
        let mut u = ProfileUpdate::default();
        let Some(saved) = &self.saved else {
            return Ok(u);
        };
        let changed = |field: Field| {
            let now = self.text(field, cx);
            (now.trim() != field.saved(saved).trim()).then(|| now.trim().to_string())
        };
        u.display_name = changed(Field::Name);
        let bio = self.text(Field::Bio, cx);
        if bio != saved.bio {
            u.bio = Some(bio);
        }
        let color = if self.custom_color {
            self.text(Field::Color, cx).trim().to_string()
        } else {
            String::new()
        };
        if self.custom_color && !is_color(&color) {
            return Err(Problem::Color);
        }
        if color != saved.name_color.clone().unwrap_or_default() {
            u.name_color = Some(color);
        }
        let accent = if self.custom_accent {
            self.text(Field::Accent, cx).trim().to_string()
        } else {
            String::new()
        };
        if self.custom_accent && !is_color(&accent) {
            return Err(Problem::Accent);
        }
        if accent != saved.profile_accent_color.clone().unwrap_or_default() {
            u.profile_accent_color = Some(accent);
        }
        u.status_emoji = changed(Field::StatusEmoji);
        u.status_text = changed(Field::StatusText);
        u.headline = changed(Field::Headline);
        u.pronouns = changed(Field::Pronouns);
        u.location = changed(Field::Location);
        u.timezone = changed(Field::Timezone);
        u.homepage_url = changed(Field::Homepage);
        if let Some(url) = &u.homepage_url
            && !url.is_empty()
            && !url.starts_with("https://")
        {
            return Err(Problem::Homepage);
        }
        if self.languages != saved.languages.as_deref().unwrap_or_default() {
            u.languages = Some(self.languages.clone());
        }
        if self.interests != saved.interests.as_deref().unwrap_or_default() {
            u.interests = Some(self.interests.clone());
        }
        if self.banner != saved.banner_url.clone().filter(|u| !u.is_empty()) {
            u.banner_url = Some(self.banner.clone().unwrap_or_default());
        }
        if self.background
            != saved
                .profile_background_url
                .clone()
                .filter(|u| !u.is_empty())
        {
            u.profile_background_url = Some(self.background.clone().unwrap_or_default());
        }
        if self.frame != saved.avatar_frame.clone().unwrap_or_default() {
            u.avatar_frame = Some(self.frame.clone());
        }
        let status_changed = u.status_emoji.is_some() || u.status_text.is_some();
        if self.clear_after != ClearAfter::Keep || status_changed {
            u.status_expires_at = Some(
                self.clear_after
                    .deadline(chrono::Local::now())
                    .unwrap_or_default(),
            );
        }
        let origin = self
            .session
            .read(cx)
            .server()
            .map(|s| s.origin())
            .unwrap_or_default();
        self.avatar.patch(&origin, &mut u);
        Ok(u)
    }

    fn dirty(&self, cx: &App) -> bool {
        self.saved.is_some()
            && match self.update(cx) {
                Ok(u) => u != ProfileUpdate::default(),
                Err(_) => true,
            }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.saved.is_none() {
            return;
        }
        let update = match self.update(cx) {
            Ok(u) => u,
            Err(problem) => {
                self.problem = Some(problem);
                cx.notify();
                return;
            }
        };
        if update == ProfileUpdate::default() {
            return;
        }
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        self.saving = true;
        self.error = None;
        self.done = false;
        cx.notify();
        let network = self.network.clone();
        let target = network.clone();
        let task = client.handle().request(move |api| async move {
            match target {
                Some(n) => api.update_network_profile(&n, &update).await,
                None => api.update_profile(&update).await,
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string());
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(Ok(member)) => {
                        client
                            .change(|w| auth::profile_saved(w, network.as_deref(), member.clone()));
                        if this.network == network {
                            this.fill(member, window, cx);
                            this.done = true;
                        }
                        super::saved(cx);
                    }
                    Ok(Err(e)) => this.error = Some(error_text(&e).into()),
                    Err(e) => this.error = Some(e.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn upload_image(&mut self, which: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(network) = self.network.clone() else {
            return;
        };
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("profile-choose-image").into()),
        });
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let Ok(Ok(Some(paths))) = picked.await else {
                    return;
                };
                let Some(path) = paths.into_iter().next() else {
                    return;
                };
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "image".into());
                let Ok(bytes) = std::fs::read(&path) else {
                    return;
                };
                let file = FileData::named(name, bytes);
                if !file.is_image() {
                    cx.update(|_, cx| {
                        this.update(cx, |v, cx| {
                            v.error = Some(t!("profile-image-open-failed").into());
                            cx.notify();
                        })
                        .ok();
                    })
                    .ok();
                    return;
                }
                cx.update(|_, cx| {
                    this.update(cx, |v, cx| {
                        v.uploading = Some(which);
                        v.error = None;
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
                let task = client.handle().request(move |api| async move {
                    api.upload_file(&network, &file, &Progress::default(), uploads::STALL)
                        .await
                        .map_err(|e| ApiError::Http {
                            status: 0,
                            detail: e.to_string(),
                        })
                });
                let result = task.await.map_err(|e| e.to_string());
                cx.update(|_, cx| {
                    this.update(cx, |v, cx| {
                        v.uploading = None;
                        match result {
                            Ok(Ok(upload)) => {
                                let url = Some(upload.url);
                                if which == "banner" {
                                    v.banner = url;
                                } else {
                                    v.background = url;
                                }
                                v.done = false;
                            }
                            Ok(Err(e)) => v.error = Some(error_text(&e).into()),
                            Err(e) => v.error = Some(e.into()),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
            })
            .detach();
    }

    fn field(&self, field: Field, theme: &Theme) -> AnyElement {
        ui::field::field(
            field.label(),
            Input::new(self.input(field))
                .id(field.id())
                .aria_label(field.label()),
            theme,
        )
        .children(self.field_problem(field, theme))
        .into_any_element()
    }

    fn field_problem(&self, field: Field, theme: &Theme) -> Option<AnyElement> {
        let text = self
            .problem
            .filter(|problem| problem.field() == field)?
            .text();
        Some(
            div()
                .id(ElementId::Name(format!("{}-problem", field.id()).into()))
                .test_support()
                .role(Role::Alert)
                .aria_label(text.clone())
                .text_sm()
                .text_color(theme.danger)
                .child(text)
                .into_any_element(),
        )
    }

    fn tag_field(
        &self,
        list: ProfileList,
        theme: &Theme,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let field = Field::List(list);
        let tags = match list {
            ProfileList::Languages => &self.languages,
            ProfileList::Interests => &self.interests,
        };
        let input = self.input(field);
        let focused = input.focus_handle(cx).is_focused(window);
        let mut editor = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .p_1p5()
            .border_1()
            .border_color(if focused { theme.accent } else { theme.border })
            .rounded(theme.radius)
            .bg(theme.input);
        let mut language_names = Vec::new();
        for (index, tag) in tags.iter().enumerate() {
            let name = match list {
                ProfileList::Languages => {
                    let name = people::language_name(tag);
                    language_names.push(name.clone());
                    name
                }
                ProfileList::Interests => tag.clone(),
            };
            let value = tag.clone();
            let view = cx.entity();
            editor = editor.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_2p5()
                    .pr_1()
                    .rounded_full()
                    .bg(theme.raised)
                    .text_sm()
                    .child(tag.clone())
                    .child(
                        Button::new(SharedString::from(format!("{}-remove-{index}", field.id())))
                            .icon(ui::icon::IconName::Close)
                            .accessibility_label(t!("profile-tag-remove", item = name))
                            .small()
                            .size(px(24.))
                            .ghost()
                            .disabled(self.saving)
                            .on_click(move |_, window, cx| {
                                view.update(cx, |view, cx| {
                                    view.tags_mut(list).retain(|tag| *tag != value);
                                    view.input(field).focus_handle(cx).focus(window, cx);
                                    view.problem = None;
                                    view.done = false;
                                    cx.notify();
                                });
                            }),
                    ),
            );
        }
        let editor = editor.child(
            div().flex_1().min_w(px(148.)).child(
                Input::new(input)
                    .id(field.id())
                    .aria_label(field.label())
                    .appearance(false)
                    .small()
                    .disabled(self.saving),
            ),
        );
        ui::field::field(field.label(), editor, theme)
            .when(!language_names.is_empty(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(language_names.join(", ")),
                )
            })
            .children(self.field_problem(field, theme))
    }
}

/// A row of choices, each `{id}-{key}`; `picked` is selected.
fn choices(
    id: &str,
    label: SharedString,
    options: Vec<(String, SharedString)>,
    picked: &str,
    theme: &Theme,
    on_pick: impl Fn(&str, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let on_pick = Rc::new(on_pick);
    let mut group = div()
        .id(ElementId::Name(id.to_string().into()))
        .role(Role::RadioGroup)
        .aria_label(label)
        .flex()
        .flex_wrap()
        .gap_1()
        .p_0p5()
        .rounded_md()
        .bg(theme.input);
    for (key, text) in options {
        let selected = key == picked;
        let hover = theme.hover;
        let on_pick = on_pick.clone();
        let value = key.clone();
        group = group.child(
            div()
                .id(ElementId::Name(format!("{id}-{key}").into()))
                .test_support()
                .role(Role::RadioButton)
                .reachable(theme)
                .aria_label(text.clone())
                .aria_selected(selected)
                .px_2()
                .py_0p5()
                .rounded_md()
                .text_sm()
                .when(selected, |d| d.bg(theme.accent).text_color(theme.on_accent))
                .when(!selected, |d| {
                    d.text_color(theme.text)
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .on_click(move |_, window, cx| on_pick(&value, window, cx))
                })
                .child(text),
        );
    }
    group.into_any_element()
}

impl Render for ProfileView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(ctx) = self.ctx.clone() else {
            return div().into_any_element();
        };
        let entity = cx.entity();
        let networks: Vec<(String, SharedString)> = {
            let session = self.session.read(cx);
            session
                .client()
                .map(|c| {
                    c.read()
                        .networks
                        .iter()
                        .map(|n| (n.id.clone(), SharedString::from(n.name.clone())))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut page = div().flex().flex_col().gap_3();

        // The server.
        if networks.is_empty() {
            page = page.child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("profile-account-only")),
            );
        } else {
            let e = entity.clone();
            page = page.child(row(
                &ctx,
                "profile-server",
                t!("profile-server"),
                Some(t!("profile-server-help").into()),
                choices(
                    "profile-network",
                    t!("profile-server").into(),
                    networks,
                    self.network.as_deref().unwrap_or_default(),
                    &theme,
                    move |id, window, cx| {
                        let id = id.to_string();
                        e.update(cx, |v, cx| v.pick_network(Some(id), window, cx));
                    },
                ),
                &theme,
            ));
        }

        let Some(saved) = self.saved.clone() else {
            let text: SharedString = match &self.load_error {
                Some(e) => e.clone().into(),
                None => t!("profile-loading").into(),
            };
            return page
                .child(
                    div()
                        .id("profile-loading")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(text.clone())
                        .text_sm()
                        .text_color(if self.load_error.is_some() {
                            theme.danger
                        } else {
                            theme.muted
                        })
                        .child(text),
                )
                .into_any_element();
        };
        let busy = self.saving || self.uploading.is_some();

        // The picture.
        let session = self.session.clone();
        let avatars = default_avatars(&session, cx);
        let (pic, origin) = {
            let s = self.session.read(cx);
            (
                preview(&self.avatar, saved.avatar_url.as_deref(), s),
                s.server().map(|s| s.origin()).unwrap_or_default(),
            )
        };
        let name_now = {
            let typed = self.text(Field::Name, cx);
            if typed.trim().is_empty() {
                self.fallback_name(&saved, cx)
            } else {
                SharedString::from(typed.trim().to_string())
            }
        };
        let mut preview_member = saved.clone();
        let color_text = self.text(Field::Color, cx);
        preview_member.name_color = (self.custom_color && is_color(color_text.trim()))
            .then(|| color_text.trim().to_string());
        let colored = crate::message::layout::settings(cx).colored_names;
        let preview_color = ui::avatar::name_color(&preview_member, colored, &theme)
            .map(theme::hex)
            .unwrap_or(theme.text);
        let selected = match &self.avatar {
            AvatarChoice::Default(a) => Some(a.id.clone()),
            AvatarChoice::Keep => {
                auth::default_avatar_for(saved.avatar_url.as_deref(), &avatars, &origin)
                    .map(|a| a.id.clone())
            }
            _ => None,
        };
        let has_picture = !matches!(pic, Picture::None);
        let e = entity.clone();
        let pick_upload = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let e = e.clone();
            pick_picture(window, cx, move |prepared, _, cx| {
                e.update(cx, |v, cx| {
                    match prepared {
                        Ok(p) => v.avatar = AvatarChoice::Upload(p),
                        Err(err) => v.error = Some(err.into()),
                    }
                    v.done = false;
                    cx.notify();
                });
            });
        };
        let e = entity.clone();
        let remove = Button::new("profile-remove-picture")
            .small()
            .link()
            .label(t!("profile-remove-image"))
            .disabled(busy)
            .on_click(move |_, _, cx| {
                e.update(cx, |v, cx| {
                    v.avatar = match v.saved.as_ref().and_then(|m| m.avatar_url.clone()) {
                        Some(u) if !u.is_empty() => AvatarChoice::Remove,
                        _ => AvatarChoice::Keep,
                    };
                    v.done = false;
                    cx.notify();
                })
            });
        let e = entity.clone();
        let defaults = (!avatars.is_empty()).then(|| {
            picker(
                "profile-default",
                &avatars,
                Marks {
                    selected: selected.as_deref(),
                    suggested: None,
                    disabled: busy,
                },
                self.session.read(cx),
                &theme,
                false,
                move |avatar, _, cx| {
                    let avatar = avatar.clone();
                    e.update(cx, |v, cx| {
                        v.avatar = AvatarChoice::Default(avatar);
                        v.done = false;
                        cx.notify();
                    })
                },
            )
        });
        let avatar_size = px(84.);
        let upload = Button::new("profile-upload-picture")
            .text()
            .size(avatar_size)
            .rounded(avatar_size / 2.)
            .accessibility_label(t!("profile-upload-image"))
            .tooltip(t!("profile-image-limits"))
            .disabled(busy)
            .on_click(pick_upload.clone())
            .child(
                div()
                    .id("profile-picture-preview")
                    .test_support()
                    .role(Role::Image)
                    .aria_label(t!("profile-picture"))
                    .child(picture(&pic, &name_now, avatar_size, &theme)),
            );
        let avatar = frame(
            &ctx,
            "profile-picture",
            t!("profile-picture").into(),
            &theme,
        )
        .flex()
        .flex_col()
        .flex_none()
        .items_center()
        .gap_1()
        .child(upload)
        .when(has_picture, |d| d.child(remove))
        .test_support();

        // Name, colour, about.
        let display_name = frame(
            &ctx,
            "profile-name",
            t!("profile-display-name").into(),
            &theme,
        )
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(t!("profile-display-name")),
        )
        .child(
            Input::new(self.input(Field::Name))
                .id(Field::Name.id())
                .aria_label(Field::Name.label()),
        )
        .child(
            div()
                .id("profile-shown-as")
                .test_support()
                .role(Role::Status)
                .aria_label(SharedString::from(t!(
                    "profile-shown-as",
                    name = name_now.clone(),
                    nick = saved.nick.clone()
                )))
                .text_sm()
                .text_color(theme.muted)
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap_1()
                .child(t!("profile-shown-as-label"))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(preview_color)
                        .child(name_now.clone()),
                )
                .child(format!("· @{}", saved.nick)),
        )
        .test_support();
        page = page
            .child(div().flex().items_start().child(avatar).child(display_name))
            .when_some(defaults, |d, picker| {
                d.child(block(
                    &ctx,
                    "profile-defaults",
                    t!("profile-default-pictures"),
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(theme.muted)
                                        .child(t!("profile-image-limits")),
                                )
                                .child(
                                    Button::new("profile-upload-own")
                                        .small()
                                        .link()
                                        .label(t!("profile-upload-own"))
                                        .disabled(busy)
                                        .on_click(pick_upload),
                                ),
                        )
                        .child(picker),
                    &theme,
                ))
            });
        let e = entity.clone();
        page = page.child(block(
            &ctx,
            "profile-color",
            t!("profile-name-color"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(choices(
                    "profile-color-mode",
                    t!("profile-name-color").into(),
                    vec![
                        ("automatic".into(), t!("profile-automatic").into()),
                        ("custom".into(), t!("profile-custom").into()),
                    ],
                    if self.custom_color {
                        "custom"
                    } else {
                        "automatic"
                    },
                    &theme,
                    move |key, window, cx| {
                        let custom = key == "custom";
                        e.update(cx, |v, cx| {
                            v.custom_color = custom;
                            if custom
                                && v.text(Field::Color, cx).trim().is_empty()
                                && let Some(color) = v.name_color.read(cx).value()
                            {
                                v.input(Field::Color).update(cx, |input, cx| {
                                    input.set_value(color.to_hex(), window, cx);
                                });
                            }
                            v.done = false;
                            v.problem = None;
                            cx.notify();
                        })
                    },
                ))
                .when(self.custom_color, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_end()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.field(Field::Color, &theme)),
                            )
                            .child(
                                div().id("profile-name-color-picker").test_support().child(
                                    ColorPicker::new(&self.name_color)
                                        .small()
                                        .accessibility_label(t!("profile-choose-name-color")),
                                ),
                            ),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("profile-name-color-help")),
                ),
            &theme,
        ));
        page = page.child(block(
            &ctx,
            "profile-bio",
            t!("profile-about-you"),
            Input::new(self.input(Field::Bio))
                .id(Field::Bio.id())
                .aria_label(Field::Bio.label()),
            &theme,
        ));

        // Status.
        let e = entity.clone();
        let mut clear: Vec<(String, SharedString)> = Vec::new();
        if saved
            .status_expires_at
            .as_deref()
            .is_some_and(|s| !s.is_empty())
        {
            clear.push(("keep".into(), ClearAfter::Keep.label()));
        }
        clear.extend(
            ClearAfter::ALL
                .iter()
                .map(|c| (c.key().to_string(), c.label())),
        );
        page = page.child(block(
            &ctx,
            "profile-status",
            t!("profile-status"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .w(px(120.))
                                .flex_none()
                                .child(self.field(Field::StatusEmoji, &theme)),
                        )
                        .child(div().flex_1().child(self.field(Field::StatusText, &theme))),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted)
                        .child(t!("profile-clear-after")),
                )
                .child(choices(
                    "profile-clear-after",
                    t!("profile-clear-after").into(),
                    clear,
                    self.clear_after.key(),
                    &theme,
                    move |key, _, cx| {
                        let choice = [ClearAfter::Keep]
                            .into_iter()
                            .chain(ClearAfter::ALL)
                            .find(|c| c.key() == key)
                            .unwrap_or(ClearAfter::Keep);
                        e.update(cx, |v, cx| {
                            v.clear_after = choice;
                            v.done = false;
                            cx.notify();
                        })
                    },
                )),
            &theme,
        ));

        // About.
        let e = entity.clone();
        let mine = Button::new("profile-timezone-mine")
            .small()
            .ghost()
            .label(t!("profile-use-mine"))
            .on_click(move |_, window, cx| {
                let zone = ircord_client::store::notify_settings::local_time_zone();
                e.update(cx, |v, cx| {
                    v.input(Field::Timezone)
                        .clone()
                        .update(cx, |s, cx| s.set_value(zone, window, cx));
                    cx.notify();
                })
            });
        page = page.child(block(
            &ctx,
            "profile-about",
            t!("profile-details"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted)
                        .child(t!("profile-details-help")),
                )
                .child(self.field(Field::Headline, &theme))
                .child(self.field(Field::Pronouns, &theme))
                .child(self.field(Field::Location, &theme))
                .child(
                    div()
                        .flex()
                        .items_end()
                        .gap_2()
                        .child(div().flex_1().child(self.field(Field::Timezone, &theme)))
                        .child(mine),
                )
                .child(self.field(Field::Homepage, &theme)),
            &theme,
        ));
        page = page.child(block(
            &ctx,
            "profile-interests",
            t!("profile-languages-and-interests"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(self.tag_field(ProfileList::Languages, &theme, window, cx))
                .child(self.tag_field(ProfileList::Interests, &theme, window, cx))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("profile-list-help")),
                ),
            &theme,
        ));

        // How the card looks.
        let image_row = |which: &'static str, current: &Option<String>, view: &Self| {
            let (label, none) = if which == "banner" {
                (t!("profile-banner"), t!("profile-no-banner"))
            } else {
                (t!("profile-background"), t!("profile-no-background"))
            };
            let e = entity.clone();
            let choose = Button::new(ElementId::Name(format!("profile-{which}-choose").into()))
                .small()
                .label(if view.uploading == Some(which) {
                    t!("profile-uploading")
                } else {
                    t!("profile-choose-image")
                })
                .disabled(busy || view.network.is_none())
                .on_click(move |_, window, cx| {
                    e.update(cx, |v, cx| v.upload_image(which, window, cx))
                });
            let e = entity.clone();
            let remove = Button::new(ElementId::Name(format!("profile-{which}-remove").into()))
                .small()
                .ghost()
                .label(t!("profile-remove"))
                .disabled(busy)
                .on_click(move |_, _, cx| {
                    e.update(cx, |v, cx| {
                        if which == "banner" {
                            v.banner = None;
                        } else {
                            v.background = None;
                        }
                        v.done = false;
                        cx.notify();
                    })
                });
            let shown: SharedString = match current {
                Some(url) => url
                    .rsplit('/')
                    .next()
                    .unwrap_or(url.as_str())
                    .to_string()
                    .into(),
                None => none.into(),
            };
            div()
                .id(ElementId::Name(format!("profile-{which}").into()))
                .test_support()
                .role(Role::Group)
                .aria_label(SharedString::from(label))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(div().text_sm().text_color(theme.text).child(label))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted)
                                .truncate()
                                .child(shown),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(choose)
                        .when(current.is_some(), |d| d.child(remove)),
                )
        };
        let banner = image_row("banner", &self.banner, self);
        let background = image_row("background", &self.background, self);
        let e = entity.clone();
        let accent_mode = choices(
            "profile-accent-mode",
            t!("profile-accent").into(),
            vec![
                ("automatic".into(), t!("profile-automatic").into()),
                ("custom".into(), t!("profile-custom").into()),
            ],
            if self.custom_accent {
                "custom"
            } else {
                "automatic"
            },
            &theme,
            move |key, _, cx| {
                let custom = key == "custom";
                e.update(cx, |v, cx| {
                    v.custom_accent = custom;
                    v.done = false;
                    v.problem = None;
                    cx.notify();
                })
            },
        );
        // The frame in the accent being picked, as the web's previews.
        let frame_color = self
            .custom_accent
            .then(|| people::parse_hex(Some(self.text(Field::Accent, cx).trim())))
            .flatten()
            .map(theme::hex)
            .unwrap_or(theme.accent);
        let e = entity.clone();
        let frame = if self.frame.is_empty() {
            "none"
        } else {
            self.frame.as_str()
        };
        let frames = choices(
            "profile-frame",
            t!("profile-frame").into(),
            FRAMES
                .iter()
                .map(|f| (f.to_string(), frame_label(f)))
                .collect(),
            frame,
            &theme,
            move |key, _, cx| {
                let key = if key == "none" {
                    String::new()
                } else {
                    key.to_string()
                };
                e.update(cx, |v, cx| {
                    v.frame = key;
                    v.done = false;
                    cx.notify();
                })
            },
        );
        page = page.child(block(
            &ctx,
            "profile-look",
            t!("profile-look"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(banner)
                .child(background)
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.text)
                        .child(t!("profile-accent")),
                )
                .child(accent_mode)
                .when(self.custom_accent, |d| {
                    d.child(self.field(Field::Accent, &theme))
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("profile-accent-help")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.text)
                        .child(t!("profile-frame")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(
                            div()
                                .id("profile-frame-preview")
                                .test_support()
                                .role(Role::Image)
                                .aria_label(t!("profile-frame-preview"))
                                .p_2()
                                .child(
                                    ui::avatar::framed(
                                        picture(&pic, &name_now, px(ui::avatar::NORMAL), &theme),
                                        people::AvatarFrame::parse(Some(&self.frame)),
                                        frame_color,
                                        theme.main,
                                        px(ui::avatar::NORMAL),
                                        "settings",
                                    )
                                    .test_support(),
                                ),
                        )
                        .child(frames),
                ),
            &theme,
        ));

        // Saving.
        let dirty = self.dirty(cx);
        let e = entity.clone();
        let save = Button::new("profile-save")
            .primary()
            .label(if self.saving {
                t!("profile-saving")
            } else {
                t!("profile-save")
            })
            .disabled(!dirty || busy)
            .on_click(move |_, window, cx| e.update(cx, |v, cx| v.save(window, cx)));
        let status: Option<(SharedString, Hsla)> = if let Some(e) = &self.error {
            Some((e.clone(), theme.danger))
        } else if let Some(p) = self.problem {
            Some((p.text(), theme.danger))
        } else if self.done {
            Some((t!("profile-saved").into(), theme.muted))
        } else {
            None
        };
        page = page.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap_3()
                .when_some(status, |d, (text, color)| {
                    d.child(
                        div()
                            .id("profile-save-status")
                            .test_support()
                            .role(Role::Status)
                            .aria_label(text.clone())
                            .text_sm()
                            .text_color(color)
                            .child(text),
                    )
                })
                .child(save),
        );
        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClearAfter, Cursor, MAX_DATA_URL, Problem, ProfileList, base64, is_color, prepare_avatar,
        validated_tag,
    };

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_picked_image_becomes_a_small_square_data_url() {
        let wide = image::RgbaImage::from_pixel(600, 300, image::Rgba([200, 30, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(wide)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let p = prepare_avatar(&png).unwrap();
        assert!(p.data_url.starts_with("data:image/png;base64,"));
        assert!(p.data_url.len() <= MAX_DATA_URL);
        let size = p.preview.size(0);
        assert_eq!((size.width.0, size.height.0), (256, 256));
        assert!(prepare_avatar(b"not an image").is_err());
    }

    #[test]
    fn colors_require_six_hex_digits() {
        assert!(is_color("#3366cc") && !is_color("3366cc") && !is_color("#33g6cc"));
    }

    #[test]
    fn profile_lists_preserve_punctuation_and_reject_repeats() {
        let langs = ProfileList::Languages;
        let interests = ProfileList::Interests;
        assert_eq!(validated_tag(langs, "en-gb", &[]), Ok("en-GB".into()));
        assert_eq!(
            validated_tag(langs, "EN-gb", &["en-GB".into()]),
            Err(Problem::Duplicate(langs))
        );
        assert_eq!(
            validated_tag(langs, "en_US", &[]),
            Err(Problem::LanguageTag)
        );
        assert_eq!(
            validated_tag(interests, "Tea, coffee", &[]),
            Ok("Tea, coffee".into())
        );
        assert_eq!(
            validated_tag(interests, "Æble", &["æble".into()]),
            Err(Problem::Duplicate(interests))
        );
        assert_eq!(
            validated_tag(interests, "🚀🚀🚀🚀🚀🚀🚀🚀🚀", &[]),
            Err(Problem::LongInterest)
        );
        let full = (0..10).map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            validated_tag(interests, "Hiking", &full),
            Err(Problem::TooMany(interests))
        );
    }

    #[test]
    fn status_deadlines_are_utc() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 21, 10, 0, 0)
            .unwrap();
        assert_eq!(ClearAfter::Keep.deadline(now), None);
        assert_eq!(ClearAfter::Never.deadline(now), Some(String::new()));
        let hour = ClearAfter::Hour1.deadline(now).unwrap();
        let at = chrono::DateTime::parse_from_rfc3339(&hour).unwrap();
        assert_eq!(at, now + chrono::Duration::hours(1));
        assert!(hour.ends_with('Z'));
    }
}
