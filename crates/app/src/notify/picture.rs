//! The sender's picture on a notification, as a file the system's toast can
//! show. The picture is cached as a 96x96 PNG; a fetch that outlives the
//! notification's wait finishes in the background for the next one.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use futures::future::{Either, select};
use gpui_kit::*;
use image::ImageFormat;
use image::imageops::FilterType;
use ircord_client::contract::Alert;
use ircord_client::store::people;

use crate::session::Session;

/// How long a cached picture is used before it is fetched again.
const FRESH: Duration = Duration::from_secs(24 * 60 * 60);
/// How long a notification waits for its picture before it shows without.
const WAIT: Duration = Duration::from_millis(800);
/// The largest avatar downloaded.
const MAX_BYTES: usize = 512 << 10;
/// The side of the cached picture, in pixels.
const SIDE: u32 = 96;
/// At most this many cached pictures are kept; the oldest go first.
const MAX_FILES: usize = 200;

/// A fixed answer, for tests of the code that calls `resolve`.
#[derive(Default)]
pub struct Fixed(pub Option<PathBuf>);
impl Global for Fixed {}

/// The sender's picture for `alert` as a PNG file, or `None`. A direct
/// conversation shows its partner, anything else the sender of the message.
/// Nothing shows when the alert hides who wrote, when nobody is found, when
/// the picture is not on this server or does not arrive within [`WAIT`].
pub fn resolve(session: &Session, alert: &Alert, cx: &mut App) -> Task<Option<PathBuf>> {
    if let Some(fixed) = cx.try_global::<Fixed>() {
        return Task::ready(fixed.0.clone());
    }
    if !alert.preview {
        return Task::ready(None);
    }
    let Some(url) = address(session, alert) else {
        return Task::ready(None);
    };
    let path = cache_path(
        &session.config.path().join("notify").join("avatars"),
        url.as_str(),
    );
    if is_fresh(&path) {
        return Task::ready(Some(path));
    }
    let runtime = session.runtime.clone();
    // The download runs to its end even when nobody waits for it any more.
    let (tx, rx) = futures::channel::oneshot::channel::<Option<PathBuf>>();
    let job = path.clone();
    runtime.spawn(async move {
        let done = download(url.to_string(), job).await;
        let _ = tx.send(done);
    });
    let pause = runtime.spawn_blocking(|| std::thread::sleep(WAIT));
    cx.background_spawn(async move {
        match select(rx, pause).await {
            Either::Left((Ok(found), _)) => found,
            _ => None,
        }
    })
}

/// The address the picture loads from, if it is on this server. The
/// partner's or the sender's record, never a picture elsewhere.
fn address(session: &Session, alert: &Alert) -> Option<url::Url> {
    let server = session.server()?.url().clone();
    let client = session.client()?;
    let w = client.read();
    let member = match people::direct_partner(&w, &alert.conversation_id) {
        Some(partner) => partner,
        None => {
            let sender = &w
                .timelines
                .get(&alert.conversation_id)?
                .find(&alert.message_id)?
                .message
                .sender_id;
            let network = &w.conversation(&alert.conversation_id)?.network_id;
            w.member(sender, network)?
        }
    };
    people::picture_url(member.avatar_url.as_deref(), &server)
}

/// `<dir>/<fnv1a of the address>.png`: the same address always has the same
/// file.
fn cache_path(dir: &Path, address: &str) -> PathBuf {
    dir.join(format!("{:016x}.png", fnv1a(address)))
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Whether a cached picture exists and is younger than [`FRESH`].
fn is_fresh(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < FRESH)
}

async fn download(url: String, path: PathBuf) -> Option<PathBuf> {
    let (status, _, bytes) = ircord_client::api::fetch(&url, MAX_BYTES).await.ok()?;
    if status != 200 {
        return None;
    }
    tokio::task::spawn_blocking(move || store(&bytes, &path))
        .await
        .ok()
        .flatten()
}

/// Decode, crop to the centred square, scale to [`SIDE`] and write the PNG
/// in one step, so a half-written file is never read.
fn store(bytes: &[u8], path: &Path) -> Option<PathBuf> {
    let picture = image::load_from_memory(bytes).ok()?;
    let side = picture.width().min(picture.height());
    if side == 0 {
        return None;
    }
    let square = picture
        .crop_imm(
            (picture.width() - side) / 2,
            (picture.height() - side) / 2,
            side,
            side,
        )
        .resize_exact(SIDE, SIDE, FilterType::Lanczos3);
    let dir = path.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    let partial = path.with_extension("partial");
    square.save_with_format(&partial, ImageFormat::Png).ok()?;
    std::fs::rename(&partial, path).ok()?;
    prune(dir);
    Some(path.to_path_buf())
}

/// Keep at most [`MAX_FILES`] pictures in `dir`, removing the oldest.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    if files.len() <= MAX_FILES {
        return;
    }
    files.sort();
    let excess = files.len() - MAX_FILES;
    for (_, old) in files.drain(..excess) {
        let _ = std::fs::remove_file(old);
    }
}
