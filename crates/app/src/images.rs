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
