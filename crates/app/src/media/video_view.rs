//! A view that draws a `Video`'s pictures: the newest one each frame while
//! playing, freeing the one before it.
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_platform::video::{State, Video};

pub struct VideoView {
    video: Box<dyn Video>,
    picture: Option<Arc<RenderImage>>,
    serial: u64,
}

impl VideoView {
    pub fn new(video: Box<dyn Video>, cx: &mut Context<Self>) -> Self {
        // Free the last picture's texture when the view goes away.
        cx.on_release(|this, cx| {
            if let Some(old) = this.picture.take() {
                cx.drop_image(old, None);
            }
        })
        .detach();
        Self {
            video,
            picture: None,
            serial: 0,
        }
    }

    pub fn video(&self) -> &dyn Video {
        self.video.as_ref()
    }
}

impl Render for VideoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(frame) = self.video.frame(self.serial) {
            self.serial = frame.serial;
            // GPUI keeps BGRA bytes in an `RgbaImage` (see `images::still`).
            if let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
            {
                let next = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
                if let Some(old) = self.picture.replace(next) {
                    // Without this every picture stays in graphics memory.
                    cx.drop_image(old, Some(window));
                }
            }
        }
        if matches!(self.video.state(), State::Playing | State::Loading) {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .bg(gpui_kit::black())
            .when_some(self.picture.clone(), |d, picture| {
                d.child(
                    img(ImageSource::Render(picture))
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                )
            })
    }
}
