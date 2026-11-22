//! GPUI loads `img(url)` through the app's HTTP client. This one runs the
//! request on the client's Tokio runtime with reqwest, the same stack the
//! rest of the app uses, and caps the size of what it downloads.

use std::sync::{Arc, RwLock};

use futures::FutureExt;
use futures::future::BoxFuture;
use gpui_kit::http_client::{self, AsyncBody, HttpClient, Response, Url};
use gpui_kit::{App, Global, SharedString};
use ircord_client::api::Api;
use tokio::runtime::Runtime;

/// Inline images larger than this are left as a link.
const MAX_IMAGE_BYTES: usize = 25 << 20;
const ARCHIVE_SCHEME: &str = "ircord-archive";

#[derive(Default)]
struct SignedInMedia {
    generation: u64,
    api: Option<Api>,
}

#[derive(Clone, Default)]
struct MediaSession(Arc<RwLock<SignedInMedia>>);

impl Global for MediaSession {}

#[derive(Clone)]
pub(crate) struct MediaReader {
    session: MediaSession,
    generation: u64,
}

impl MediaReader {
    pub(crate) async fn fetch(
        &self,
        url: &str,
        max: usize,
    ) -> Result<(u16, Option<String>, Vec<u8>), String> {
        let api = {
            let state = self.session.0.read().unwrap();
            (state.generation == self.generation)
                .then(|| state.api.clone())
                .flatten()
        }
        .ok_or_else(|| "the media session ended".to_string())?;
        let response = api.fetch_chat_file(url, max).await;
        if self.session.0.read().unwrap().generation != self.generation {
            return Err("the media session ended".into());
        }
        response
    }
}

pub(crate) fn set_media_session(api: Option<Api>, cx: &mut App) {
    if let Some(media) = cx.try_global::<MediaSession>() {
        let mut state = media.0.write().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.api = api;
    }
}

pub(crate) fn media_reader(cx: &App) -> Option<MediaReader> {
    let session = cx.try_global::<MediaSession>()?.clone();
    let generation = {
        let state = session.0.read().unwrap();
        state.api.as_ref()?;
        state.generation
    };
    Some(MediaReader {
        session,
        generation,
    })
}

/// Give message images a session-specific key in GPUI's image cache.
/// The alias never goes over the network or into copy-link actions.
pub(crate) fn media_uri(url: &str, reader: Option<&MediaReader>) -> SharedString {
    let Some(reader) = reader else {
        return url.to_owned().into();
    };
    let mut alias = Url::parse(&format!("{ARCHIVE_SCHEME}://media/{}", reader.generation))
        .expect("the message image alias is a valid URI");
    alias.query_pairs_mut().append_pair("url", url);
    alias.to_string().into()
}

pub struct ImageClient {
    runtime: Arc<Runtime>,
    media: MediaSession,
}

impl ImageClient {
    pub fn install(runtime: Arc<Runtime>, cx: &mut App) {
        let media = MediaSession::default();
        cx.set_global(media.clone());
        cx.set_http_client(Arc::new(Self { runtime, media }));
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
        let media = self.media.clone();
        let task = self.runtime.spawn(async move {
            if url.starts_with("ircord-archive:") {
                let alias = Url::parse(&url).map_err(|e| e.to_string())?;
                let generation = alias
                    .path()
                    .trim_start_matches('/')
                    .parse::<u64>()
                    .map_err(|e| e.to_string())?;
                let original = alias
                    .query_pairs()
                    .find(|(key, _)| key == "url")
                    .map(|(_, value)| value.into_owned())
                    .ok_or_else(|| "missing message image URL".to_string())?;
                let reader = MediaReader {
                    session: media,
                    generation,
                };
                reader.fetch(&original, MAX_IMAGE_BYTES).await
            } else {
                ircord_client::api::fetch(&url, MAX_IMAGE_BYTES).await
            }
        });
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

#[derive(Default)]
struct AvatarBackdrops(
    std::collections::HashMap<gpui_kit::SharedString, Arc<gpui_kit::RenderImage>>,
);

impl gpui_kit::Global for AvatarBackdrops {}

/// A small softened copy of the first avatar frame, used behind a profile.
/// Keep this separate from the decoded original used by the avatar itself.
pub fn avatar_backdrop(url: gpui_kit::SharedString) -> gpui_kit::ImageSource {
    use gpui_kit::{ImgResourceLoader, RenderImage, Resource, SharedUri};
    gpui_kit::ImageSource::Custom(Arc::new(move |window, cx| {
        if let Some(cached) = cx
            .try_global::<AvatarBackdrops>()
            .and_then(|images| images.0.get(&url))
        {
            return Some(Ok(cached.clone()));
        }
        let resource = Resource::Uri(SharedUri::from(url.to_string()));
        let decoded = match window.use_asset::<ImgResourceLoader>(&resource, cx)? {
            Ok(image) => image,
            Err(error) => return Some(Err(error)),
        };
        let size = decoded.size(0);
        // The decoder and the atlas both use BGRA; the filters preserve channel order.
        let pixels = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
            size.width.0 as u32,
            size.height.0 as u32,
            decoded.as_bytes(0)?,
        )?;
        let small = image::imageops::thumbnail(&pixels, 96, 96);
        let blurred = image::imageops::blur(&small, 8.);
        let backdrop = Arc::new(RenderImage::new(vec![image::Frame::new(blurred)]));
        let images = cx.default_global::<AvatarBackdrops>();
        if images.0.len() >= 32 {
            images.0.clear();
        }
        images.0.insert(url.clone(), backdrop.clone());
        Some(Ok(backdrop))
    }))
}
