//! Draw the background sample in Theme Studio.

use std::io::Cursor;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use image::{ImageReader, Limits, Rgba, RgbaImage};
use ircord_client::Client;
use ircord_client::store::appearance::{
    Backdrop, BackdropFit as Fit, BackdropPosition as Position,
};

#[derive(Default)]
pub(crate) struct Wallpaper {
    selected: Option<(Weak<Client>, String)>,
    source: Option<Arc<RgbaImage>>,
    texture: Option<(Layout, Arc<RenderImage>)>,
    download: Option<(tokio::task::AbortHandle, Task<()>)>,
    generation: u64,
    drawing: bool,
}

impl Drop for Wallpaper {
    fn drop(&mut self) {
        if let Some((abort, _)) = &self.download {
            abort.abort();
        }
    }
}

impl Wallpaper {
    pub(crate) fn select(
        &mut self,
        selection: Option<(&Rc<Client>, &Backdrop)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (&self.selected, selection) {
            (None, None) => return,
            (Some((scope, path)), Some((client, bg)))
                if scope.ptr_eq(&Rc::downgrade(client)) && path == &bg.path =>
            {
                return;
            }
            _ => {}
        }
        self.clear(Some(window), cx);
        let Some((client, bg)) = selection else {
            return;
        };
        self.selected = Some((Rc::downgrade(client), bg.path.clone()));
        let path = bg.path.clone();
        let request = client.request(move |api| async move { api.theme_background(&path).await });
        let abort = request.abort_handle();
        let executor = cx.background_executor().clone();
        let generation = self.generation;
        let task = cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(bytes)) = request.await else {
                return;
            };
            let source = executor
                .spawn(async move { decode(&bytes).map(Arc::new) })
                .await;
            this.update_in(cx, |this, window, _| {
                if this.generation == generation {
                    this.source = source;
                    window.refresh();
                }
            })
            .ok();
        });
        self.download = Some((abort, task));
    }

    pub(crate) fn clear(&mut self, window: Option<&mut Window>, cx: &mut App) {
        if self.selected.take().is_none() {
            return;
        }
        self.source = None;
        self.generation = self.generation.wrapping_add(1);
        if let Some((abort, _)) = self.download.take() {
            abort.abort();
        }
        self.replace_texture(None, window, cx);
    }

    fn replace_texture(
        &mut self,
        new: Option<(Layout, Arc<RenderImage>)>,
        window: Option<&mut Window>,
        cx: &mut App,
    ) {
        if let Some((_, old)) = std::mem::replace(&mut self.texture, new) {
            cx.drop_image(old, window);
        }
    }

    fn texture(
        &mut self,
        layout: Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        if !self.drawing
            && !self
                .texture
                .as_ref()
                .is_some_and(|(rendered, _)| *rendered == layout)
            && let Some(source) = self.source.clone()
        {
            self.drawing = true;
            let generation = self.generation;
            let rendered = cx.background_spawn(async move {
                let mut pixels = compose(&source, layout);
                // GPUI's texture atlas expects BGRA pixels.
                for pixel in pixels.pixels_mut() {
                    pixel.0.swap(0, 2);
                }
                Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
            });
            cx.spawn_in(window, async move |this, cx| {
                let image = rendered.await;
                this.update_in(cx, |this, window, cx| {
                    this.drawing = false;
                    if this.generation == generation {
                        this.replace_texture(Some((layout, image)), Some(window), cx);
                    }
                    window.refresh();
                })
                .ok();
            })
            .detach();
        }
        self.texture.as_ref().map(|(_, image)| image.clone())
    }
}

pub(crate) fn layer(
    state: Entity<Wallpaper>,
    settings: &Backdrop,
    opacity: f32,
    corners: Corners<Pixels>,
) -> impl IntoElement + use<> {
    let (fit, position, blur) = (settings.fit, settings.position, settings.blur as u8);
    canvas(
        move |bounds, window, cx| {
            let width = f32::from(bounds.size.width).ceil() as u32;
            let height = f32::from(bounds.size.height).ceil() as u32;
            let layout = Layout {
                width,
                height,
                fit,
                position,
                blur,
            };
            if width == 0 || height == 0 {
                return None;
            }
            state.update(cx, |state, cx| state.texture(layout, window, cx))
        },
        move |bounds, image, window, _| {
            if let Some(image) = image {
                let _ = window.paint_image(bounds, bounds, corners, image, 0, false);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .opacity(opacity)
}

pub(crate) fn decode(bytes: &[u8]) -> Option<RgbaImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(64 << 20);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    Some(image.into_rgba8())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    width: u32,
    height: u32,
    fit: Fit,
    position: Position,
    blur: u8,
}

fn compose(source: &RgbaImage, layout: Layout) -> RgbaImage {
    // Bound the texture allocation on large or high-density displays.
    let density = (2048. / layout.width.max(layout.height) as f32).min(1.);
    let width = (layout.width as f32 * density).round().max(1.) as u32;
    let height = (layout.height as f32 * density).round().max(1.) as u32;
    let sigma = f32::from(layout.blur) * density;
    let bleed = (2. * sigma).ceil() as u32;
    let (w, h) = (width + 2 * bleed, height + 2 * bleed);
    let (sw, sh) = (source.width() as f32, source.height() as f32);
    let (x_ratio, y_ratio) = (w as f32 / sw, h as f32 / sh);
    let scale = match layout.fit {
        Fit::Contain => x_ratio.min(y_ratio),
        Fit::Tile => density,
        _ => x_ratio.max(y_ratio),
    };
    let (x, y) = match layout.position {
        Position::Top => (0.5, 0.),
        Position::Bottom => (0.5, 1.),
        Position::Left => (0., 0.5),
        Position::Right => (1., 0.5),
        _ => (0.5, 0.5),
    };
    let (left, top) = ((w as f32 - sw * scale) * x, (h as f32 - sh * scale) * y);
    let position = |pixel: u32, start, extent: f32| {
        let coordinate: f32 = (pixel as f32 + 0.5 - start) / scale;
        if layout.fit == Fit::Tile {
            Some(coordinate.rem_euclid(extent))
        } else {
            (0. ..extent).contains(&coordinate).then_some(coordinate)
        }
    };
    let mut composed = RgbaImage::from_fn(w, h, |x, y| {
        let Some(x) = position(x, left, sw) else {
            return Rgba([0; 4]);
        };
        let Some(y) = position(y, top, sh) else {
            return Rgba([0; 4]);
        };
        let x = (x - 0.5).clamp(0., sw - 1.);
        let y = (y - 0.5).clamp(0., sh - 1.);
        image::imageops::interpolate_bilinear(source, x, y).unwrap_or(Rgba([0; 4]))
    });
    if bleed > 0 {
        for Rgba([r, g, b, a]) in composed.pixels_mut() {
            for channel in [r, g, b] {
                *channel = (u16::from(*channel) * u16::from(*a) / 255) as u8;
            }
        }
        let mut blurred = image::imageops::fast_blur(&composed, sigma);
        for Rgba([r, g, b, a]) in blurred.pixels_mut() {
            if *a > 0 {
                for channel in [r, g, b] {
                    *channel = (u16::from(*channel) * 255 / u16::from(*a)).min(255) as u8;
                }
            }
        }
        return image::imageops::crop_imm(&blurred, bleed, bleed, width, height).to_image();
    }
    composed
}

#[cfg(test)]
mod tests {
    use super::{Fit, Layout, Position, Rgba, RgbaImage, compose};

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

    #[test]
    fn background_fit_positions_the_source_or_repeats_it() {
        let source = RgbaImage::from_fn(2, 1, |x, _| if x == 0 { RED } else { BLUE });
        let mut layout = Layout {
            width: 1,
            height: 1,
            fit: Fit::Cover,
            position: Position::Left,
            blur: 0,
        };
        assert_eq!(*compose(&source, layout).get_pixel(0, 0), RED);
        layout.position = Position::Right;
        assert_eq!(*compose(&source, layout).get_pixel(0, 0), BLUE);

        layout.width = 4;
        layout.fit = Fit::Tile;
        assert_eq!(
            compose(&source, layout)
                .pixels()
                .copied()
                .collect::<Vec<_>>(),
            [RED, BLUE, RED, BLUE]
        );

        layout.width = 2;
        layout.height = 3;
        layout.position = Position::Bottom;
        layout.fit = Fit::Contain;
        let contain = compose(&source, layout);
        assert_eq!(contain.get_pixel(0, 0).0[3], 0);
        assert_eq!(*contain.get_pixel(1, 2), BLUE);
    }

    #[test]
    fn blur_leaves_the_visible_extent_intact() {
        let source = RgbaImage::from_pixel(40, 40, RED);
        let layout = Layout {
            width: 28,
            height: 20,
            fit: Fit::Cover,
            position: Position::Center,
            blur: 4,
        };
        let output = compose(&source, layout);
        assert_eq!(output.dimensions(), (28, 20));
        assert!(output.pixels().all(|pixel| *pixel == RED));
    }
}
