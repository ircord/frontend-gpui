//! The picture a personal theme draws behind the app (the web's "Personal
//! theme background"). The theme names an upload on the signed-in server;
//! this module reads it with the session (never `img(url)`), decodes it once
//! off the UI thread, and keeps one GPU texture per picture and blur, plus a
//! frosted copy. The views ask [`get`] and, when it has a picture, draw it
//! with [`layer`] or [`frosted`].
//!
//! Placement `conversation` puts it behind the open conversation only;
//! `app` puts it behind the whole window and turns the shell's own surfaces
//! (the top bar, the conversation list, the card) into frosted panels over
//! it, as the web's glass panels are (`backdrop-filter: blur`). GPUI has no
//! live backdrop blur, so the picture is blurred once, off the UI thread,
//! and a panel draws its own region of that copy under its tint. With no
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
/// The height of the theme editor's preview; its width is at most twice this.
const PREVIEW_SIDE: u32 = 120;
/// The width the web's blur radius is measured against.
const BLUR_REFERENCE: f32 = 1600.;
/// The blur of a frosted panel, in the web's pixels (the glass panels of
/// castle.css use `blur(12px)`), on top of the theme's own blur.
const FROST: f32 = 12.;

/// A picture that is ready to draw.
#[derive(Clone)]
pub struct Drawn {
    pub spec: Backdrop,
    image: Arc<RenderImage>,
    frost: Arc<RenderImage>,
}

/// The textures made for one picture and blur.
struct Textures {
    path: String,
    blur: u32,
    image: Arc<RenderImage>,
    frost: Arc<RenderImage>,
}

#[derive(Default)]
struct State {
    /// The picture asked for and the client it was asked from.
    requested: Option<(String, usize)>,
    /// The decoded picture, before blur.
    source: Option<(String, Arc<image::RgbaImage>)>,
    /// The textures drawn, for this path and blur.
    textures: Option<Textures>,
    /// The (path, blur) being built off the UI thread.
    building: Option<(String, u32)>,
    spec: Option<Backdrop>,
    generation: u64,
}

impl Global for State {}

/// Forget the textures, freeing them on the GPU.
fn release(cx: &mut App) {
    if let Some(old) = cx.default_global::<State>().textures.take() {
        cx.drop_image(old.image, None);
        cx.drop_image(old.frost, None);
    }
}

/// How many times the picture was painted, sharp and frosted, since the app
/// started; the tests read it to see what a frame drew.
#[derive(Default)]
struct Painted {
    sharp: usize,
    frosted: usize,
}

impl Global for Painted {}

/// `(sharp, frosted)` paints of the picture so far.
pub fn painted(cx: &App) -> (usize, usize) {
    cx.try_global::<Painted>()
        .map_or((0, 0), |p| (p.sharp, p.frosted))
}

/// The picture to draw now: the theme's, once it has been read.
pub fn get(cx: &App) -> Option<Drawn> {
    let state = cx.try_global::<State>()?;
    let spec = state.spec.clone()?;
    let t = state.textures.as_ref()?;
    (t.path == spec.path && t.blur == spec.blur as u32).then(|| Drawn {
        spec,
        image: t.image.clone(),
        frost: t.frost.clone(),
    })
}

/// The colour of a surface the app-wide picture shows through.
pub fn shell(theme: &Theme, cx: &App) -> Hsla {
    match get(cx) {
        Some(drawn) if drawn.spec.placement == BackdropPlacement::App => hsla(0., 0., 0., 0.),
        _ => theme.sidebar,
    }
}

/// The frosted fill of a shell panel (the top bar, the conversation list)
/// over an app-wide picture: the sidebar colour at the panel opacity over
/// the blurred picture. `None` when nothing is drawn behind the window.
pub fn panel(theme: &Theme, corners: Corners<Pixels>, cx: &App) -> Option<AnyElement> {
    let drawn = get(cx).filter(|d| d.spec.placement == BackdropPlacement::App)?;
    Some(frosted(&drawn, theme.sidebar, drawn.spec.panel, corners).into_any_element())
}

/// Keep the picture the compiled theme asks for read and ready. Called
/// whenever the appearance is applied.
pub(super) fn sync(spec: Option<&Backdrop>, client: Option<Rc<Client>>, cx: &mut App) {
    let state = cx.default_global::<State>();
    state.spec = spec.cloned();
    let Some(spec) = spec else {
        state.requested = None;
        state.source = None;
        state.building = None;
        // A load still running is no longer wanted.
        state.generation += 1;
        release(cx);
        return;
    };
    let blur = spec.blur as u32;
    // Same picture, new blur: make the textures again from what is read.
    if let Some((path, source)) = state.source.clone().filter(|(p, _)| *p == spec.path) {
        let ready = state
            .textures
            .as_ref()
            .is_some_and(|t| t.path == path && t.blur == blur);
        if !ready && state.building.as_ref() != Some(&(path.clone(), blur)) {
            build(path, source, blur, cx);
        }
        return;
    }
    let Some(client) = client else {
        return;
    };
    let key = (spec.path.clone(), Rc::as_ptr(&client) as usize);
    if state.requested.as_ref() == Some(&key) {
        return;
    }
    state.requested = Some(key);
    state.generation += 1;
    let generation = state.generation;
    let path = spec.path.clone();
    let wanted = path.clone();
    let task = client.request(move |api| async move { api.theme_picture(&wanted).await });
    cx.spawn(async move |cx| {
        // A failed read is forgotten, so the next `sync` (every reconnect
        // applies the appearance again) asks again.
        let failed = |cx: &mut AsyncApp| {
            cx.update(|cx| {
                let state = cx.default_global::<State>();
                if state.generation == generation {
                    state.requested = None;
                }
            })
        };
        let bytes = match task.await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) => {
                log::warn!("could not read the theme's background: {e}");
                failed(cx);
                return;
            }
            Err(e) => {
                log::warn!("could not read the theme's background: {e}");
                failed(cx);
                return;
            }
        };
        let decoded = cx.background_spawn(async move { decode(&bytes) }).await;
        let Some(decoded) = decoded else {
            log::warn!("the theme's background is not a picture this app can read");
            failed(cx);
            return;
        };
        cx.update(|cx| {
            let state = cx.default_global::<State>();
            if state.generation != generation {
                return;
            }
            let source = Arc::new(decoded);
            state.source = Some((path.clone(), source.clone()));
            let blur = state.spec.as_ref().map_or(0, |s| s.blur as u32);
            build(path, source, blur, cx);
        });
    })
    .detach();
}

/// Make the textures for `path` at `blur` off the UI thread, and keep them
/// when they are still wanted.
fn build(path: String, source: Arc<image::RgbaImage>, blur: u32, cx: &mut App) {
    let state = cx.default_global::<State>();
    state.building = Some((path.clone(), blur));
    let generation = state.generation;
    cx.spawn(async move |cx| {
        let made = cx
            .background_spawn(async move {
                (render(&source, blur), render(&source, blur + FROST as u32))
            })
            .await;
        cx.update(|cx| {
            let state = cx.default_global::<State>();
            if state.building.as_ref() == Some(&(path.clone(), blur)) {
                state.building = None;
            }
            if state.generation != generation {
                return;
            }
            let wanted = state
                .spec
                .as_ref()
                .is_some_and(|s| s.path == path && s.blur as u32 == blur);
            if !wanted {
                // The theme changed meanwhile; `sync` builds what it now wants.
                if let Some(spec) = state.spec.clone()
                    && let Some((p, source)) = state.source.clone().filter(|(p, _)| *p == spec.path)
                    && state.building.is_none()
                {
                    build(p, source, spec.blur as u32, cx);
                }
                return;
            }
            let (image, frost) = made;
            let image = Arc::new(RenderImage::new(vec![image::Frame::new(image)]));
            let frost = Arc::new(RenderImage::new(vec![image::Frame::new(frost)]));
            release(cx);
            cx.default_global::<State>().textures = Some(Textures {
                path,
                blur,
                image,
                frost,
            });
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

/// What the theme editor shows of a picture just chosen or read from the
/// account: a small copy to draw, and its dark and light tones, measured on
/// the picture shrunk to 24 by 24 pixels as the web does. Blocking; run it
/// off the UI thread.
pub fn preview(bytes: &[u8]) -> Option<(Arc<RenderImage>, Option<[String; 2]>)> {
    let source = decode(bytes)?;
    let tones = ircord_client::store::appearance::measure_tones(
        image::imageops::resize(&source, 24, 24, FilterType::Triangle).as_raw(),
    );
    let small = image::DynamicImage::ImageRgba8(source)
        .resize(PREVIEW_SIDE * 2, PREVIEW_SIDE, FilterType::Triangle)
        .to_rgba8();
    let image = Arc::new(RenderImage::new(vec![image::Frame::new(render(&small, 0))]));
    Some((image, tones))
}

/// `source` with `blur` pixels of blur, as GPUI's atlas takes it
/// (premultiplied BGRA).
fn render(source: &image::RgbaImage, blur: u32) -> image::RgbaImage {
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
    for pixel in picture.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let premultiply = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
        pixel.0 = [premultiply(b), premultiply(g), premultiply(r), a];
    }
    picture
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

/// Paint `image` placed in `area` per `spec`, showing only `clip`.
fn paint_picture(
    window: &mut Window,
    spec: &Backdrop,
    image: &Arc<RenderImage>,
    clip: Bounds<Pixels>,
    area: Bounds<Pixels>,
    corners: Corners<Pixels>,
) {
    let natural = image.size(0);
    let natural = size(px(natural.width.0 as f32), px(natural.height.0 as f32));
    if natural.width <= px(0.) || natural.height <= px(0.) {
        return;
    }
    let first = placed(spec, natural, area);
    if spec.fit != BackdropFit::Tile {
        let _ = window.paint_image(clip, first, corners, image.clone(), 0, false);
        return;
    }
    // Tiles from the area's top-left, at the image's own size.
    let mut y = first.origin.y;
    while y > clip.origin.y {
        y -= natural.height;
    }
    while y < clip.bottom() {
        let mut x = area.origin.x;
        while x + natural.width <= clip.origin.x {
            x += natural.width;
        }
        while x < clip.right() {
            // Only the tiles at the clip's top corners are rounded.
            let top = y <= clip.origin.y;
            let tile_corners = Corners {
                top_left: if top && x <= clip.origin.x {
                    corners.top_left
                } else {
                    px(0.)
                },
                top_right: if top && x + natural.width >= clip.right() {
                    corners.top_right
                } else {
                    px(0.)
                },
                ..Default::default()
            };
            let _ = window.paint_image(
                clip,
                Bounds::new(point(x, y), natural),
                tile_corners,
                image.clone(),
                0,
                false,
            );
            x += natural.width;
        }
        y += natural.height;
    }
}

/// The picture over its parent, covered by `cover` (the surface it sits
/// behind) at `amount`: the web's picture at opacity `1 - amount`. For the
/// `conversation` placement and for the window-wide layer; the parent should
/// clip it.
pub fn layer(
    drawn: &Drawn,
    cover: Hsla,
    amount: f32,
    corners: Corners<Pixels>,
) -> impl IntoElement {
    paint_layer(drawn, false, vec![(cover, amount)], corners)
}

/// A frosted panel over an app-wide picture: the panel's own region of the
/// blurred copy, placed as the window-wide picture is, under `tint` at
/// `amount`. It fills its parent, which should clip it.
pub fn frosted(
    drawn: &Drawn,
    tint: Hsla,
    amount: f32,
    corners: Corners<Pixels>,
) -> impl IntoElement {
    paint_layer(drawn, true, vec![(tint, amount)], corners)
}

/// The conversation card over an app-wide picture: the web's card is the
/// main colour at the dim over the picture already covered by the panel
/// colour, so the frosted copy gets both covers.
pub fn frosted_card(drawn: &Drawn, theme: &Theme, corners: Corners<Pixels>) -> impl IntoElement {
    let covers = vec![
        (theme.sidebar, drawn.spec.panel),
        (theme.main, drawn.spec.dim),
    ];
    paint_layer(drawn, true, covers, corners)
}

fn paint_layer(
    drawn: &Drawn,
    frost: bool,
    covers: Vec<(Hsla, f32)>,
    corners: Corners<Pixels>,
) -> impl IntoElement {
    let spec = drawn.spec.clone();
    let image = if frost {
        drawn.frost.clone()
    } else {
        drawn.image.clone()
    };
    div().absolute().top_0().left_0().size_full().child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let painted = cx.default_global::<Painted>();
                if frost {
                    painted.frosted += 1;
                } else {
                    painted.sharp += 1;
                }
                // A frosted panel shows its region of the window's picture.
                let area = if frost {
                    Bounds::new(point(px(0.), px(0.)), window.viewport_size())
                } else {
                    bounds
                };
                paint_picture(window, &spec, &image, bounds, area, corners);
                // The cover follows the card's rounded corners too.
                for (cover, amount) in &covers {
                    window.paint_quad(fill(bounds, cover.opacity(*amount)).corner_radii(corners));
                }
            },
        )
        .size_full(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn a_frosted_copy_is_blurrier_than_the_picture() {
        // Left half black, right half white.
        let edge = image::RgbaImage::from_fn(1280, 100, |x, _| {
            let v = if x < 640 { 0 } else { 255 };
            image::Rgba([v, v, v, 255])
        });
        // The blue channel is first in BGRA; all channels are equal here.
        let step = |p: &image::RgbaImage| {
            let at = |x: u32| i32::from(p.get_pixel(x, 50)[0]);
            (at(644) - at(636)).abs()
        };
        let sharp = render(&edge, 0);
        let frosted = render(&edge, FROST as u32);
        assert_eq!(step(&sharp), 255);
        assert!(step(&frosted) < 100, "{}", step(&frosted));
        // Premultiplied BGRA: a half-transparent red pixel.
        let red = image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 0, 0, 128]));
        assert_eq!(render(&red, 0).get_pixel(0, 0).0, [0, 0, 100, 128]);
    }
}
