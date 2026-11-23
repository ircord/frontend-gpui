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
const MAX_PIXELS: usize = 24_000_000;
/// One picture holds at most this many device pixels; past it, it is drawn
/// at a lower resolution.
const MAX_PICTURE_PIXELS: f32 = 8_000_000.;
/// Layouts kept; past it the cache starts again.
const MAX_LAYOUTS: usize = 128;
/// The picture's height inside the frame before it scrolls, as code does.
const MAX_HEIGHT: f32 = 24.;

/// Draw the diagram `id` from `source`, or say why it cannot be drawn.
pub fn render(
    id: &str,
    source: &str,
    theme: &Theme,
    mono: SharedString,
    window: &mut Window,
    cx: &mut App,
) -> Result<AnyElement, SharedString> {
    let look = diagram_theme(theme);
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
            Some(picture_for(key, laid, width.get(), window, cx))
        }
        _ => None,
    };
    Ok(frame(
        id,
        source,
        theme,
        mono,
        width,
        showing_source,
        picture,
    ))
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
}

#[derive(Default)]
struct Diagrams {
    layouts: HashMap<u64, Layout>,
    /// By layout and device width.
    pictures: HashMap<(u64, u32), Picture>,
    /// Device pixels held by the ready pictures.
    pixels: usize,
    /// Diagrams whose source is shown in place of the picture.
    showing_source: HashSet<String>,
}

impl Global for Diagrams {}

fn layout_for(key: u64, source: &str, look: &DiagramTheme, cx: &mut App) -> Layout {
    let diagrams = cx.default_global::<Diagrams>();
    if let Some(known) = diagrams.layouts.get(&key) {
        return known.clone();
    }
    if diagrams.layouts.len() >= MAX_LAYOUTS {
        diagrams.layouts.clear();
    }
    diagrams.layouts.insert(key, Layout::Pending);
    let (source, look) = (source.to_string(), look.clone());
    let task = cx.background_spawn(async move { lay_out(&source, &look) });
    cx.spawn(async move |cx| {
        let laid = task.await;
        cx.update(|cx| {
            let state = match laid {
                Ok(laid) => Layout::Ready(Arc::new(laid)),
                Err(e) => Layout::Failed(e),
            };
            cx.default_global::<Diagrams>().layouts.insert(key, state);
            cx.refresh_windows();
        });
    })
    .detach();
    Layout::Pending
}

fn picture_for(
    key: u64,
    laid: &Arc<Laid>,
    available: f32,
    window: &Window,
    cx: &mut App,
) -> Picture {
    // The natural size, or the available width when that is narrower,
    // rounded down so resizing the window by a pixel draws nothing new.
    let shown = if laid.width <= available {
        laid.width
    } else {
        (available / 8.).floor().max(1.) * 8.
    };
    let height = laid.height * shown / laid.width;
    let mut scale = window.scale_factor();
    let device = shown * height * scale * scale;
    if device > MAX_PICTURE_PIXELS {
        scale *= (MAX_PICTURE_PIXELS / device).sqrt();
    }
    let (w, h) = (
        (shown * scale).round().max(1.) as u32,
        (height * scale).round().max(1.) as u32,
    );
    let size = size(px(shown), px(height));
    let diagrams = cx.default_global::<Diagrams>();
    if let Some(known) = diagrams.pictures.get(&(key, w)) {
        return known.clone();
    }
    diagrams.pictures.insert((key, w), Picture::Pending);
    let laid = laid.clone();
    let task = cx.background_spawn(async move { rasterise(&laid, w, h) });
    cx.spawn(async move |cx| {
        let picture = task.await;
        cx.update(|cx| {
            let diagrams = cx.default_global::<Diagrams>();
            let Some(picture) = picture else {
                diagrams.pictures.remove(&(key, w));
                return;
            };
            let pixels = (w * h) as usize;
            if diagrams.pixels + pixels > MAX_PIXELS {
                diagrams
                    .pictures
                    .retain(|_, p| matches!(p, Picture::Pending));
                diagrams.pixels = 0;
            }
            diagrams.pixels += pixels;
            diagrams
                .pictures
                .insert((key, w), Picture::Ready(picture, size));
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

/// The frame: the label and the two buttons over the picture, its source,
/// or a line saying it is being drawn.
fn frame(
    id: &str,
    source: &str,
    theme: &Theme,
    mono: SharedString,
    width: Rc<Cell<f32>>,
    showing_source: bool,
    picture: Option<Picture>,
) -> AnyElement {
    use gpui_kit::TestSupportExt;
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
    let content: AnyElement = if showing_source {
        div()
            .id(ElementId::Name(format!("{id}-source").into()))
            .test_support()
            .font_family(mono)
            .text_sm()
            .whitespace_nowrap()
            .child(source.to_string())
            .into_any_element()
    } else {
        match picture {
            Some(Picture::Ready(image, size)) => div()
                .id(ElementId::Name(format!("{id}-picture").into()))
                .test_support()
                .role(Role::Image)
                .aria_label(t!("text-diagram"))
                .w(size.width)
                .h(size.height)
                .child(img(ImageSource::Render(image)).w(size.width).h(size.height))
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
                .child(div().flex().child(toggle).child(copy)),
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
