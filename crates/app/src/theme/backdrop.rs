//! The picture a personal theme draws behind the app (the web's "Personal
//! theme background"). The theme names an upload on the signed-in server;
//! this module reads it with the session (never `img(url)`), decodes it once
//! off the UI thread, and keeps one GPU texture per picture and blur. The
//! views ask [`get`] and, when it has a picture, [`layer`] draws it.
//!
//! Placement `conversation` puts it behind the open conversation only;
//! `app` puts it behind the whole window and turns the shell's own surfaces
//! (the conversation list, the card's colour) translucent over it. With no
//! picture drawn, [`get`] is `None` and every surface keeps its colour.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::*;
use image::imageops::FilterType;
use ircord_client::Client;
use ircord_client::store::appearance::{
    Backdrop, BackdropFit, BackdropPlacement, BackdropPosition,
};

use super::Theme;

/// The longest side kept of a picture without blur. A window rarely draws
/// more, and the GPU scales it on resize, so nothing is decoded again.
const MAX_SIDE: u32 = 2560;
/// With blur the picture is first made smaller, so blurring it is cheap.
const MAX_SIDE_BLURRED: u32 = 1280;
/// The width the web's blur radius is measured against.
const BLUR_REFERENCE: f32 = 1600.;

/// A picture that is ready to draw.
#[derive(Clone)]
pub struct Drawn {
    pub spec: Backdrop,
    image: Arc<RenderImage>,
}

#[derive(Default)]
struct State {
    /// The picture asked for and the client it was asked from.
    requested: Option<(String, usize)>,
    /// The decoded picture, before blur.
    source: Option<(String, Arc<image::RgbaImage>)>,
    /// The texture drawn, for this path and blur.
    texture: Option<(String, u32, Arc<RenderImage>)>,
    spec: Option<Backdrop>,
    generation: u64,
}

impl Global for State {}

/// The picture to draw now: the theme's, once it has been read.
pub fn get(cx: &App) -> Option<Drawn> {
    let state = cx.try_global::<State>()?;
    let spec = state.spec.clone()?;
    let (path, blur, image) = state.texture.as_ref()?;
    (*path == spec.path && *blur == spec.blur as u32).then(|| Drawn {
        spec,
        image: image.clone(),
    })
}

/// The colour of a surface the app-wide picture shows through.
pub fn shell(theme: &Theme, cx: &App) -> Hsla {
    match get(cx) {
        Some(drawn) if drawn.spec.placement == BackdropPlacement::App => hsla(0., 0., 0., 0.),
        _ => theme.sidebar,
    }
}

/// Keep the picture the compiled theme asks for read and ready. Called
/// whenever the appearance is applied.
pub(super) fn sync(spec: Option<&Backdrop>, client: Option<Rc<Client>>, cx: &mut App) {
    let state = cx.default_global::<State>();
    state.spec = spec.cloned();
    let Some(spec) = spec else {
        state.requested = None;
        state.source = None;
        state.texture = None;
        return;
    };
    let blur = spec.blur as u32;
    // Same picture, new blur: decode it again from what is already read.
    if state.source.as_ref().is_some_and(|(p, _)| *p == spec.path)
        && state
            .texture
            .as_ref()
            .is_none_or(|(p, b, _)| *p != spec.path || *b != blur)
    {
        let (path, source) = state.source.clone().expect("checked above");
        let texture = texture(&source, blur);
        let old = state.texture.replace((path, blur, texture));
        if let Some((_, _, old)) = old {
            cx.drop_image(old, None);
        }
        cx.refresh_windows();
        return;
    }
    let Some(client) = client else {
        return;
    };
    let key = (spec.path.clone(), Rc::as_ptr(&client) as usize);
    if state.requested.as_ref() == Some(&key)
        || state.source.as_ref().is_some_and(|(p, _)| *p == key.0)
    {
        return;
    }
    state.requested = Some(key);
    state.generation += 1;
    let generation = state.generation;
    let path = spec.path.clone();
    let wanted = path.clone();
    let task = client.request(move |api| async move { api.theme_picture(&wanted).await });
    cx.spawn(async move |cx| {
        let bytes = match task.await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                log::warn!("could not read the theme's background: {e}");
                return;
            }
            Err(e) => {
                log::warn!("could not read the theme's background: {e}");
                return;
            }
        };
        let decoded = cx.background_spawn(async move { decode(&bytes) }).await;
        let Some(decoded) = decoded else {
            log::warn!("the theme's background is not a picture this app can read");
            return;
        };
        cx.update(|cx| {
            let state = cx.default_global::<State>();
            if state.generation != generation {
                return;
            }
            let blur = state.spec.as_ref().map_or(0, |s| s.blur as u32);
            let source = Arc::new(decoded);
            let texture = texture(&source, blur);
            state.source = Some((path.clone(), source));
            let old = state.texture.replace((path, blur, texture));
            if let Some((_, _, old)) = old {
                cx.drop_image(old, None);
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Decode `bytes` and make them no larger than the window will ever draw.
fn decode(bytes: &[u8]) -> Option<image::RgbaImage> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let longest = decoded.width().max(decoded.height());
    let decoded = if longest > MAX_SIDE {
        decoded.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle)
    } else {
        decoded
    };
    Some(decoded.to_rgba8())
}

/// The GPU texture for `source` with `blur` pixels of blur.
fn texture(source: &image::RgbaImage, blur: u32) -> Arc<RenderImage> {
    let mut picture = if blur == 0 {
        source.clone()
    } else {
        let longest = source.width().max(source.height());
        let small = if longest > MAX_SIDE_BLURRED {
            let scale = MAX_SIDE_BLURRED as f32 / longest as f32;
            image::imageops::resize(
                source,
                ((source.width() as f32 * scale).round() as u32).max(1),
                ((source.height() as f32 * scale).round() as u32).max(1),
                FilterType::Triangle,
            )
        } else {
            source.clone()
        };
        let sigma = blur as f32 * small.width() as f32 / BLUR_REFERENCE;
        image::imageops::blur(&small, sigma.max(0.5))
    };
    // GPUI's texture atlas expects premultiplied BGRA.
    for pixel in picture.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let premultiply = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
        pixel.0 = [premultiply(b), premultiply(g), premultiply(r), a];
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(picture)]))
}

/// Where the picture sits in `area`.
fn placed(spec: &Backdrop, natural: Size<Pixels>, area: Bounds<Pixels>) -> Bounds<Pixels> {
    let ratio = natural.width / natural.height;
    let (width, height) = match spec.fit {
        BackdropFit::Tile => (natural.width, natural.height),
        fit => {
            let by_width = area.size.width / area.size.height < ratio;
            let fill = matches!(fit, BackdropFit::Cover) == by_width;
            if fill {
                (area.size.height * ratio, area.size.height)
            } else {
                (area.size.width, area.size.width / ratio)
            }
        }
    };
    let free_x = area.size.width - width;
    let free_y = area.size.height - height;
    let (fx, fy) = match spec.position {
        BackdropPosition::Center => (0.5, 0.5),
        BackdropPosition::Top => (0.5, 0.),
        BackdropPosition::Bottom => (0.5, 1.),
        BackdropPosition::Left => (0., 0.5),
        BackdropPosition::Right => (1., 0.5),
    };
    Bounds::new(
        point(area.origin.x + free_x * fx, area.origin.y + free_y * fy),
        size(width, height),
    )
}

/// The picture over `area`, covered by `cover` (the surface it sits behind)
/// at `amount`: the web's picture at opacity `1 - amount`. It fills its
/// parent, which should clip it.
pub fn layer(
    drawn: &Drawn,
    cover: Hsla,
    amount: f32,
    corners: Corners<Pixels>,
) -> impl IntoElement {
    let spec = drawn.spec.clone();
    let image = drawn.image.clone();
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let natural = image.size(0);
                    let natural = size(px(natural.width.0 as f32), px(natural.height.0 as f32));
                    if natural.width <= px(0.) || natural.height <= px(0.) {
                        return;
                    }
                    let first = placed(&spec, natural, bounds);
                    if spec.fit == BackdropFit::Tile {
                        let mut y = first.origin.y;
                        while y > bounds.origin.y {
                            y -= natural.height;
                        }
                        while y < bounds.bottom() {
                            let mut x = bounds.origin.x;
                            while x < bounds.right() {
                                let _ = window.paint_image(
                                    bounds,
                                    Bounds::new(point(x, y), natural),
                                    corners,
                                    image.clone(),
                                    0,
                                    false,
                                );
                                x += natural.width;
                            }
                            y += natural.height;
                        }
                    } else {
                        let _ = window.paint_image(bounds, first, corners, image.clone(), 0, false);
                    }
                },
            )
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(cover.opacity(amount)),
        )
}
