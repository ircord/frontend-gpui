//! GPUI loads `img(url)` through the app's HTTP client. This one runs the
//! request on the client's Tokio runtime with reqwest, the same stack the
//! rest of the app uses, and caps the size of what it downloads.

use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use gpui_kit::http_client::{self, AsyncBody, HttpClient, Response, Url};
use tokio::runtime::Runtime;

/// Inline images larger than this are left as a link.
const MAX_IMAGE_BYTES: usize = 25 << 20;

pub struct ImageClient {
    runtime: Arc<Runtime>,
}

impl ImageClient {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }
}

impl HttpClient for ImageClient {
    fn user_agent(&self) -> Option<&http_client::http::HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: http_client::Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let url = req.uri().to_string();
        let task = self
            .runtime
            .spawn(async move { ircord_client::api::fetch(&url, MAX_IMAGE_BYTES).await });
        async move {
            let (status, content_type, body) = task.await?.map_err(anyhow::Error::msg)?;
            let mut response = Response::builder().status(status);
            if let Some(content_type) = content_type {
                response = response.header("content-type", content_type);
            }
            Ok(response.body(AsyncBody::from(body))?)
        }
        .boxed()
    }
}

/// Stills made so far, by URL, so each is uploaded to the GPU once.
#[derive(Default)]
struct Stills(std::collections::HashMap<gpui_kit::SharedString, Arc<gpui_kit::RenderImage>>);

impl gpui_kit::Global for Stills {}

/// At most this many stills are kept; past it the cache starts again.
const MAX_STILLS: usize = 256;

/// An animated picture held on its first frame: GPUI's `img` always plays
/// every frame, so a GIF that should stand still is drawn from this source.
/// It loads the picture through the same cache as `img(url)`.
pub fn still(url: gpui_kit::SharedString) -> gpui_kit::ImageSource {
    use gpui_kit::{ImgResourceLoader, RenderImage, Resource, SharedUri};
    gpui_kit::ImageSource::Custom(Arc::new(move |window, cx| {
        if let Some(cached) = cx.try_global::<Stills>().and_then(|s| s.0.get(&url)) {
            return Some(Ok(cached.clone()));
        }
        let resource = Resource::Uri(SharedUri::from(url.to_string()));
        let full = match window.use_asset::<ImgResourceLoader>(&resource, cx)? {
            Ok(full) => full,
            Err(e) => return Some(Err(e)),
        };
        if full.frame_count() <= 1 {
            return Some(Ok(full));
        }
        let size = full.size(0);
        let bytes = full.as_bytes(0)?.to_vec();
        let buffer = image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes)?;
        let still = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        let stills = cx.default_global::<Stills>();
        if stills.0.len() >= MAX_STILLS {
            stills.0.clear();
        }
        stills.0.insert(url.clone(), still.clone());
        Some(Ok(still))
    }))
}
