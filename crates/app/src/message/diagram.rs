//! A `mermaid` fence drawn as a picture. `ircord_client::format::diagram`
//! lays the source out as an SVG; this file measures its text in the app's
//! own fonts, turns the SVG into a picture off the main thread, keeps the
//! pictures, and draws the frame: the label, "Show source" and copy.
//!
//! The picture is drawn at the width it is shown at, never wider than the
//! message: the width comes from the frame's layout in the frame before.
//! Until a picture is ready the frame says so; a source that cannot be
//! drawn goes back to the body as an error, which shows it as code.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::format::diagram::{self as layout, DiagramError, DiagramTheme, Measure};

use crate::theme::Theme;
use crate::ui;

/// The families the app embeds (`theme::fonts`); any other interface font
/// draws diagrams in Inter, so measuring and drawing use the same face.
const FAMILIES: &[&str] = &[
    "Atkinson Hyperlegible Next",
    "Geist",
    "IBM Plex Sans",
    "Inter",
    "Lexend",
    "Literata",
    "Manrope",
    "Nunito",
    "Source Sans 3",
    "Source Serif 4",
];
const DEFAULT_FAMILY: &str = "Inter";

/// Pictures are dropped once together they hold more device pixels than this.
const MAX_PIXELS: usize = 40_000_000;
/// One picture in a message holds at most this many device pixels; past
/// it, it is drawn at a lower resolution.
const MAX_PICTURE_PIXELS: f32 = 8_000_000.;
/// The same for the fullscreen picture, which is drawn at natural size.
const MAX_FULLSCREEN_PIXELS: f32 = 16_000_000.;
/// Layouts kept; past it the half asked for longest ago is forgotten.
const MAX_LAYOUTS: usize = 128;
/// Label widths kept per font; past it the cache starts again.
const MAX_WIDTHS: usize = 4096;
/// The picture's height inside the frame before it scrolls, as code does.
const MAX_HEIGHT: f32 = 24.;

/// Draw the diagram `id` from `source`, or say why it cannot be drawn.
pub fn render(
    id: &str,
    source: &str,
    theme: &Theme,
    mono: SharedString,
    source_tag: super::copy::Tag,
    window: &mut Window,
    cx: &mut App,
) -> Result<AnyElement, SharedString> {
    // A theme change fades the window's colours; the layout takes the final
    // ones at once instead of laying out again on every frame of the fade.
    let look = diagram_theme(crate::theme::settled(cx));
    let key = hash(&(source, &look));
    let laid = match layout_for(key, source, &look, cx) {
        Layout::Failed(e) => return Err(note(&e)),
        Layout::Pending => None,
        Layout::Ready(laid) => Some(laid),
    };
    let width = window
        .use_keyed_state(ElementId::Name(format!("{id}-width").into()), cx, |_, _| {
            Rc::new(Cell::new(0f32))
        })
        .read(cx)
        .clone();
    let showing_source = cx
        .try_global::<Diagrams>()
        .is_some_and(|d| d.showing_source.contains(id));
    let picture = match (&laid, showing_source) {
        (Some(laid), false) if width.get() > 0. => {
            let shown = fitted(laid, width.get());
            Some(picture_for(key, Use::Message, laid, shown, window, cx))
        }
        _ => None,
    };
    let kind = layout::diagram_type(source)
        .unwrap_or("mermaid")
        .to_string();
    let picture_label: SharedString = t!("text-diagram-picture", kind = kind).into();
    let fullscreen = laid.map(|laid| (key, laid));
    let shown = Shown {
        width,
        showing_source,
        picture,
        picture_label,
        fullscreen,
    };
    Ok(frame(id, source, theme, mono, source_tag, shown))
}

/// The theme's colours and font, as the layout takes them.
fn diagram_theme(theme: &Theme) -> DiagramTheme {
    let font = FAMILIES
        .iter()
        .find(|f| **f == theme.font.as_ref())
        .copied()
        .unwrap_or(DEFAULT_FAMILY);
    DiagramTheme {
        dark: theme.dark,
        font: font.to_string(),
        text: css(theme.text),
        node: css(theme.raised),
        border: css(theme.control_border),
        line: css(theme.muted),
        secondary: css(theme.panel),
        tertiary: css(theme.hover),
        background: css(theme.code),
    }
}

fn css(color: Hsla) -> String {
    let c = Rgba::from(color);
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
}

fn hash(value: &impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

fn note(e: &DiagramError) -> SharedString {
    match e {
        DiagramError::TooLong => t!("text-diagram-too-long").into(),
        DiagramError::TooLarge => t!("text-diagram-too-large").into(),
        DiagramError::Unsupported => t!("text-diagram-unsupported").into(),
        DiagramError::Timeout => t!("text-diagram-timeout").into(),
        DiagramError::Parse(error) => {
            let first = error.lines().next().unwrap_or_default();
            t!("text-diagram-invalid", error = first.to_string()).into()
        }
        DiagramError::Failed(_) => t!("text-diagram-failed").into(),
    }
}

/// A laid-out diagram: the SVG with its text turned into shapes, and its
/// natural size in pixels.
struct Laid {
    tree: resvg::usvg::Tree,
    width: f32,
    height: f32,
}

#[derive(Clone)]
enum Layout {
    Pending,
    Ready(Arc<Laid>),
    Failed(DiagramError),
}

#[derive(Clone)]
enum Picture {
    Pending,
    Ready(Arc<RenderImage>, Size<Pixels>),
    Failed,
}

/// Where a picture is shown. Each diagram keeps one picture per use: the
/// one for its current size there.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Use {
    Message,
    Fullscreen,
}

/// The picture a diagram has for one use, and the logical width and scale
/// it was asked for.
struct Entry {
    shown: f32,
    scale: f32,
    picture: Picture,
}

#[derive(Default)]
struct Diagrams {
    /// Layouts, with the tick each was last asked for.
    layouts: HashMap<u64, (Layout, u64)>,
    tick: u64,
    pictures: HashMap<(u64, Use), Entry>,
    /// Device pixels held by the ready pictures.
    pixels: usize,
    /// Diagrams whose source is shown in place of the picture.
    showing_source: HashSet<String>,
}

impl Global for Diagrams {}

impl Diagrams {
    /// Forget the picture under `at`, handing back its image so the caller
    /// takes it out of the windows' sprite atlas too.
    fn remove(&mut self, at: &(u64, Use)) -> Option<Arc<RenderImage>> {
        match self.pictures.remove(at)?.picture {
            Picture::Ready(image, _) => {
                self.pixels = self.pixels.saturating_sub(pixels(&image));
                Some(image)
            }
            _ => None,
        }
    }
}

fn pixels(image: &RenderImage) -> usize {
    let size = image.size(0);
    size.width.0.max(0) as usize * size.height.0.max(0) as usize
}

/// Take pictures out of GPUI's sprite atlas, where they would otherwise stay
/// for the life of the process.
fn drop_images(images: Vec<Arc<RenderImage>>, mut window: Option<&mut Window>, cx: &mut App) {
    for image in images {
        cx.drop_image(image, window.as_deref_mut());
    }
}

fn layout_for(key: u64, source: &str, look: &DiagramTheme, cx: &mut App) -> Layout {
    let diagrams = cx.default_global::<Diagrams>();
    diagrams.tick += 1;
    let tick = diagrams.tick;
    if let Some((known, used)) = diagrams.layouts.get_mut(&key) {
        *used = tick;
        return known.clone();
    }
    let mut dropped = Vec::new();
    if diagrams.layouts.len() >= MAX_LAYOUTS {
        // Forget the half asked for longest ago, and their pictures; what
        // is on screen was asked for in the last frames.
        let mut ages: Vec<u64> = diagrams.layouts.values().map(|(_, used)| *used).collect();
        ages.sort_unstable();
        let cut = ages[ages.len() / 2];
        let old: Vec<u64> = diagrams
            .layouts
            .iter()
            .filter(|(_, (_, used))| *used < cut)
            .map(|(k, _)| *k)
            .collect();
        for k in old {
            diagrams.layouts.remove(&k);
            for use_ in [Use::Message, Use::Fullscreen] {
                dropped.extend(diagrams.remove(&(k, use_)));
            }
        }
    }
    diagrams.layouts.insert(key, (Layout::Pending, tick));
    drop_images(dropped, None, cx);
    let (source, look) = (source.to_string(), look.clone());
    let task = cx.background_spawn(async move {
        // A panic in the layout must not leave the frame drawing forever.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lay_out(&source, &look)))
            .unwrap_or_else(|_| Err(DiagramError::Failed("the layout panicked".into())))
    });
    cx.spawn(async move |cx| {
        let laid = task.await;
        cx.update(|cx| {
            let state = match laid {
                Ok(laid) => Layout::Ready(Arc::new(laid)),
                Err(e) => Layout::Failed(e),
            };
            if let Some(entry) = cx.default_global::<Diagrams>().layouts.get_mut(&key) {
                entry.0 = state;
            }
            cx.refresh_windows();
        });
    })
    .detach();
    Layout::Pending
}

/// The width a diagram is shown at in `available`: its natural width, or
/// the available width when that is narrower, rounded down so resizing the
/// window by a pixel draws nothing new.
fn fitted(laid: &Laid, available: f32) -> f32 {
    if laid.width <= available {
        laid.width
    } else {
        (available / 8.).floor().max(1.) * 8.
    }
}

/// The picture of layout `key` for `use_`, shown `shown` pixels wide and
/// drawn for the window's scale. A picture for another size or scale is
/// replaced, and taken out of the sprite atlas.
fn picture_for(
    key: u64,
    use_: Use,
    laid: &Arc<Laid>,
    shown: f32,
    window: &mut Window,
    cx: &mut App,
) -> Picture {
    let max_pixels = match use_ {
        Use::Message => MAX_PICTURE_PIXELS,
        Use::Fullscreen => MAX_FULLSCREEN_PIXELS,
    };
    let scale = window.scale_factor();
    let height = laid.height * shown / laid.width;
    let mut raster = scale;
    let device = shown * height * scale * scale;
    if device > max_pixels {
        raster *= (max_pixels / device).sqrt();
    }
    let (w, h) = (
        (shown * raster).round().max(1.) as u32,
        (height * raster).round().max(1.) as u32,
    );
    let size = size(px(shown), px(height));
    let at = (key, use_);
    let diagrams = cx.default_global::<Diagrams>();
    if let Some(entry) = diagrams.pictures.get(&at)
        && entry.shown == shown
        && entry.scale == scale
    {
        return entry.picture.clone();
    }
    let old: Vec<_> = diagrams.remove(&at).into_iter().collect();
    diagrams.pictures.insert(
        at,
        Entry {
            shown,
            scale,
            picture: Picture::Pending,
        },
    );
    drop_images(old, Some(window), cx);
    let laid = laid.clone();
    let task = cx.background_spawn(async move {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rasterise(&laid, w, h)))
            .ok()
            .flatten()
    });
    cx.spawn(async move |cx| {
        let picture = task.await;
        cx.update(|cx| {
            let diagrams = cx.default_global::<Diagrams>();
            let current = diagrams
                .pictures
                .get(&at)
                .is_some_and(|e| e.shown == shown && e.scale == scale);
            let mut dropped = Vec::new();
            match picture {
                // Asked for another size meanwhile, or forgotten.
                Some(image) if !current => dropped.push(image),
                None if !current => {}
                None => {
                    if let Some(entry) = diagrams.pictures.get_mut(&at) {
                        entry.picture = Picture::Failed;
                    }
                }
                Some(image) => {
                    let new = pixels(&image);
                    if diagrams.pixels + new > MAX_PIXELS {
                        let others: Vec<_> = diagrams
                            .pictures
                            .iter()
                            .filter(|(k, e)| **k != at && matches!(e.picture, Picture::Ready(..)))
                            .map(|(k, _)| *k)
                            .collect();
                        for k in others {
                            dropped.extend(diagrams.remove(&k));
                        }
                    }
                    diagrams.pixels += new;
                    if let Some(entry) = diagrams.pictures.get_mut(&at) {
                        entry.picture = Picture::Ready(image, size);
                    }
                }
            }
            drop_images(dropped, None, cx);
            cx.refresh_windows();
        });
    })
    .detach();
    Picture::Pending
}

/// The source as a laid-out SVG with its text in the app's fonts. Blocking.
fn lay_out(source: &str, look: &DiagramTheme) -> Result<Laid, DiagramError> {
    let fonts = fonts();
    let svg = layout::render(source, look, fonts.measure(&look.font))?.svg;
    let options = resvg::usvg::Options {
        fontdb: fonts.db.clone(),
        font_family: look.font.clone(),
        resources_dir: None,
        // A diagram never reads a file or an address, whatever it names.
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(&svg, &options)
        .map_err(|e| DiagramError::Failed(e.to_string()))?;
    let natural = tree.size();
    if natural.width() < 1. || natural.height() < 1. {
        return Err(DiagramError::Failed("empty diagram".into()));
    }
    Ok(Laid {
        width: natural.width(),
        height: natural.height(),
        tree,
    })
}

/// The diagram as a `w`×`h` picture, in the BGRA GPUI draws.
fn rasterise(laid: &Laid, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    let transform =
        resvg::tiny_skia::Transform::from_scale(w as f32 / laid.width, h as f32 / laid.height);
    resvg::render(&laid.tree, transform, &mut pixmap.as_mut());
    let mut data = pixmap.take();
    for pixel in data.as_chunks_mut::<4>().0 {
        // Premultiplied RGBA to straight BGRA.
        pixel.swap(0, 2);
        let a = pixel[3];
        if a > 0 && a < 255 {
            for c in &mut pixel[..3] {
                *c = ((*c as u16 * 255 + a as u16 / 2) / a as u16).min(255) as u8;
            }
        }
    }
    let buffer = image::RgbaImage::from_raw(w, h, data)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// The embedded fonts and the system's (for emoji and scripts the
/// embedded ones lack), and the measure for each family.
struct Fonts {
    db: Arc<resvg::usvg::fontdb::Database>,
    measures: Mutex<HashMap<String, Measure>>,
}

fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        for file in crate::theme::fonts::files() {
            db.load_font_data(file.to_vec());
        }
        db.set_sans_serif_family(DEFAULT_FAMILY);
        Fonts {
            db: Arc::new(db),
            measures: Mutex::new(HashMap::new()),
        }
    })
}

impl Fonts {
    /// Text widths in `family`, shaped as the picture will draw them.
    fn measure(&self, family: &str) -> Measure {
        let mut measures = self.measures.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = measures.get(family) {
            return m.clone();
        }
        let face = |weight| {
            use resvg::usvg::fontdb::{Family, Query};
            let id = self.db.query(&Query {
                families: &[Family::Name(family), Family::SansSerif],
                weight,
                ..Default::default()
            })?;
            self.db
                .with_face_data(id, |data, index| (Arc::<[u8]>::from(data), index))
        };
        let regular = face(resvg::usvg::fontdb::Weight::NORMAL);
        let bold = face(resvg::usvg::fontdb::Weight::BOLD).or_else(|| regular.clone());
        let widths: Mutex<HashMap<(String, bool), f64>> = Mutex::new(HashMap::new());
        let measure: Measure = Arc::new(move |text, size, is_bold| {
            if text.is_empty() {
                return 0.;
            }
            let key = (text.to_string(), is_bold);
            let mut widths = widths.lock().unwrap_or_else(|e| e.into_inner());
            if widths.len() >= MAX_WIDTHS {
                widths.clear();
            }
            let em = *widths.entry(key).or_insert_with(|| {
                let face = if is_bold { &bold } else { &regular };
                face.as_ref()
                    .and_then(|(data, index)| advance_em(data, *index, text))
                    .unwrap_or(text.chars().count() as f64 * 0.55)
            });
            em * size.max(1.)
        });
        measures.insert(family.to_string(), measure.clone());
        measure
    }
}

/// The width of `text` in ems.
fn advance_em(data: &[u8], index: u32, text: &str) -> Option<f64> {
    let face = rustybuzz::Face::from_slice(data, index)?;
    let units = face.units_per_em();
    if units == 0 {
        return None;
    }
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    let shaped = rustybuzz::shape(&face, &[], buffer);
    let advance: i64 = shaped
        .glyph_positions()
        .iter()
        .map(|g| i64::from(g.x_advance))
        .sum();
    Some(advance as f64 / f64::from(units))
}

/// What one frame shows.
struct Shown {
    /// The width the picture may take, measured in the frame before.
    width: Rc<Cell<f32>>,
    showing_source: bool,
    picture: Option<Picture>,
    /// What the picture shows, for a screen reader ("flowchart diagram").
    picture_label: SharedString,
    /// The laid-out diagram, once there is one to open fullscreen.
    fullscreen: Option<(u64, Arc<Laid>)>,
}

/// The frame: the label and its buttons over the picture, its source, or a
/// line saying it is being drawn.
fn frame(
    id: &str,
    source: &str,
    theme: &Theme,
    mono: SharedString,
    source_tag: super::copy::Tag,
    shown: Shown,
) -> AnyElement {
    use gpui_kit::TestSupportExt;
    let Shown {
        width,
        showing_source,
        picture,
        picture_label,
        fullscreen,
    } = shown;
    let copy = {
        let source = source.to_string();
        ui::icon::button(
            ElementId::Name(format!("{id}-copy").into()),
            ui::icon::IconName::Copy,
            t!("text-diagram-copy"),
            theme,
            move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(source.clone()));
                ui::toast::success(window, cx, t!("text-diagram-copied"));
            },
        )
    };
    let toggle = {
        let key = id.to_string();
        let (icon, label) = if showing_source {
            (ui::icon::IconName::Eye, t!("text-diagram-show-diagram"))
        } else {
            (ui::icon::IconName::FileText, t!("text-diagram-show-source"))
        };
        ui::icon::button(
            ElementId::Name(format!("{id}-toggle").into()),
            icon,
            label,
            theme,
            move |_, _, cx| {
                let shown = &mut cx.default_global::<Diagrams>().showing_source;
                if !shown.remove(&key) {
                    shown.insert(key.clone());
                }
                cx.refresh_windows();
            },
        )
    };
    let fullscreen = fullscreen.map(|(key, laid)| {
        let label = picture_label.clone();
        ui::icon::button(
            ElementId::Name(format!("{id}-fullscreen").into()),
            ui::icon::IconName::Maximize,
            t!("text-diagram-fullscreen"),
            theme,
            move |_, window, cx| open_fullscreen(key, laid.clone(), label.clone(), window, cx),
        )
    });
    let content: AnyElement = if showing_source {
        // Selectable, as every other text of a message.
        div()
            .id(ElementId::Name(format!("{id}-source").into()))
            .test_support()
            .font_family(mono)
            .text_sm()
            .whitespace_nowrap()
            .child(
                super::body::RichText::new(
                    ElementId::Name(format!("{id}-source-text").into()),
                    source.to_string(),
                )
                .copy(source_tag),
            )
            .into_any_element()
    } else {
        match picture {
            Some(Picture::Ready(image, size)) => div()
                .id(ElementId::Name(format!("{id}-picture").into()))
                .test_support()
                .role(Role::Image)
                .aria_label(picture_label)
                .w(size.width)
                .h(size.height)
                .child(img(ImageSource::Render(image)).w(size.width).h(size.height))
                .into_any_element(),
            Some(Picture::Failed) => div()
                .id(ElementId::Name(format!("{id}-failed").into()))
                .test_support()
                .text_xs()
                .text_color(theme.faint)
                .child(t!("text-diagram-failed"))
                .into_any_element(),
            _ => div()
                .text_xs()
                .text_color(theme.faint)
                .child(t!("text-diagram-drawing"))
                .into_any_element(),
        }
    };
    // Measures the width the picture may take, for the next frame.
    let measure = canvas(
        move |bounds, window, _| {
            let w = f32::from(bounds.size.width);
            if (width.get() - w).abs() >= 0.5 {
                width.set(w);
                window.refresh();
            }
        },
        |_, _, _, _| {},
    )
    .w_full()
    .h(px(0.));
    div()
        .id(ElementId::Name(id.to_string().into()))
        .test_support()
        .role(Role::Group)
        .aria_label(t!("text-diagram"))
        .max_w_full()
        .my_0p5()
        .rounded_md()
        .border_1()
        .border_color(theme.rail)
        .bg(theme.code)
        .not_italic()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pl_2p5()
                .pr_1()
                .pt_0p5()
                .child(div().text_xs().text_color(theme.faint).child("mermaid"))
                .child(div().flex().children(fullscreen).child(toggle).child(copy)),
        )
        .child(
            div()
                .id(ElementId::Name(format!("{id}-scroll").into()))
                .overflow_scroll()
                .max_h(rems(MAX_HEIGHT))
                .px_2p5()
                .pb_2()
                .child(measure)
                .child(content),
        )
        .into_any_element()
}

/// The diagram at its natural size in a dialog filling the window, as the
/// image viewer opens a picture: it scrolls both ways when it is larger,
/// and Escape closes it. The picture is drawn for this size, not enlarged
/// from the message's.
fn open_fullscreen(
    key: u64,
    laid: Arc<Laid>,
    label: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt;
    let size = window.viewport_size();
    let width = (size.width - px(64.)).max(px(320.));
    window.open_dialog(cx, move |dialog, window, cx| {
        dialog.width(width).title(t!("text-diagram")).child(stage(
            key,
            &laid,
            label.clone(),
            window,
            cx,
        ))
    });
}

fn stage(
    key: u64,
    laid: &Arc<Laid>,
    label: SharedString,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    use gpui_kit::TestSupportExt;
    let theme = crate::theme::get(cx).clone();
    let height = (window.viewport_size().height - px(220.)).max(px(200.));
    let picture = match picture_for(key, Use::Fullscreen, laid, laid.width, window, cx) {
        Picture::Ready(image, size) => div()
            .id("diagram-viewer-picture")
            .test_support()
            .role(Role::Image)
            .aria_label(label)
            .flex_none()
            .w(size.width)
            .h(size.height)
            .child(img(ImageSource::Render(image)).w(size.width).h(size.height))
            .into_any_element(),
        Picture::Failed => div()
            .m_auto()
            .text_xs()
            .text_color(theme.faint)
            .child(t!("text-diagram-failed"))
            .into_any_element(),
        Picture::Pending => div()
            .m_auto()
            .text_xs()
            .text_color(theme.faint)
            .child(t!("text-diagram-drawing"))
            .into_any_element(),
    };
    let fits = px(laid.width) < window.viewport_size().width - px(112.) && px(laid.height) < height;
    div()
        .id("diagram-viewer-stage")
        .test_support()
        .h(height)
        .w_full()
        .rounded_md()
        .bg(theme.code)
        .overflow_scroll()
        .flex()
        .when(fits, |d| d.items_center().justify_center())
        .child(picture)
        .into_any_element()
}

/// The pictures held now, for tests that check they leave the sprite atlas.
#[doc(hidden)]
pub fn held_pictures(cx: &App) -> Vec<Arc<RenderImage>> {
    cx.try_global::<Diagrams>()
        .map(|d| {
            d.pictures
                .values()
                .filter_map(|e| match &e.picture {
                    Picture::Ready(image, _) => Some(image.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}
