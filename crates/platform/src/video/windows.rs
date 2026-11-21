//! Windows: `Windows.Media.Playback.MediaPlayer` in frame-server mode. The
//! player decodes with Media Foundation and plays the sound itself; each
//! time a picture is due it is copied to a Direct3D texture of the size
//! asked for and read back.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Foundation::{TimeSpan, TypedEventHandler, Uri};
use windows::Graphics::DirectX::Direct3D11::IDirect3DSurface;
use windows::Media::Core::MediaSource;
use windows::Media::Playback::{MediaPlaybackState, MediaPlayer, MediaPlayerFailedEventArgs};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGISurface;
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11SurfaceFromDXGISurface;
use windows::core::{HSTRING, IInspectable, Interface, Result};

use super::{Frame, State, Video};

/// Set to true once a person has watched video play on Windows (see the plan).
pub const PROVEN: bool = false;

struct Shared {
    frame: Option<Frame>,
    failed: bool,
    ended: bool,
}

struct Target {
    context: ID3D11DeviceContext,
    texture: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    surface: IDirect3DSurface,
    width: u32,
    height: u32,
}
// The context is only used inside the player's frame callback, one call at a time.
unsafe impl Send for Target {}
unsafe impl Sync for Target {}

pub struct WinVideo {
    player: MediaPlayer,
    shared: Arc<Mutex<Shared>>,
}

fn device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let (mut device, mut context) = (None, None);
        let made = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        if made.is_ok()
            && let (Some(device), Some(context)) = (device, context)
        {
            return Ok((device, context));
        }
    }
    Err(windows::core::Error::from_hresult(windows::core::HRESULT(
        0x887A0004u32 as i32, // DXGI_ERROR_UNSUPPORTED
    )))
}

fn target(width: u32, height: u32) -> Result<Target> {
    let (device, context) = device()?;
    let mut desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
    let texture: ID3D11Texture2D = texture.unwrap();
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    let mut staging = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging))? };
    let staging: ID3D11Texture2D = staging.unwrap();
    let dxgi: IDXGISurface = texture.cast()?;
    let surface: IDirect3DSurface =
        unsafe { CreateDirect3D11SurfaceFromDXGISurface(&dxgi)? }.cast()?;
    Ok(Target {
        context,
        texture,
        staging,
        surface,
        width,
        height,
    })
}

impl Target {
    fn read(&self, player: &MediaPlayer, serial: u64) -> Result<Frame> {
        player.CopyFrameToVideoSurface(&self.surface)?;
        let mut bgra = vec![0u8; (self.width * self.height * 4) as usize];
        unsafe {
            self.context.CopyResource(&self.staging, &self.texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = (self.width * 4) as usize;
            for y in 0..self.height as usize {
                let from = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                std::ptr::copy_nonoverlapping(from, bgra.as_mut_ptr().add(y * row), row);
            }
            self.context.Unmap(&self.staging, 0);
        }
        Ok(Frame {
            width: self.width,
            height: self.height,
            bgra,
            serial,
        })
    }
}

pub fn open(url: &str, width: u32, height: u32) -> std::result::Result<WinVideo, String> {
    open_inner(url, width.max(2), height.max(2)).map_err(|e| e.to_string())
}

fn open_inner(url: &str, width: u32, height: u32) -> Result<WinVideo> {
    let target = target(width, height)?;
    let player = MediaPlayer::new()?;
    player.SetAutoPlay(false)?;
    player.SetIsVideoFrameServerEnabled(true)?;
    let shared = Arc::new(Mutex::new(Shared {
        frame: None,
        failed: false,
        ended: false,
    }));
    let slot = shared.clone();
    player.VideoFrameAvailable(&TypedEventHandler::<MediaPlayer, IInspectable>::new(
        move |player, _| {
            let Some(player) = player.as_ref() else {
                return Ok(());
            };
            let mut shared = slot.lock().unwrap();
            let serial = shared.frame.as_ref().map_or(1, |f| f.serial + 1);
            match target.read(player, serial) {
                Ok(frame) => shared.frame = Some(frame),
                Err(e) => log::warn!("video frame: {e}"),
            }
            Ok(())
        },
    ))?;
    let slot = shared.clone();
    player.MediaFailed(
        &TypedEventHandler::<MediaPlayer, MediaPlayerFailedEventArgs>::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                log::warn!("video failed: {}", args.ErrorMessage().unwrap_or_default());
            }
            slot.lock().unwrap().failed = true;
            Ok(())
        }),
    )?;
    let slot = shared.clone();
    player.MediaEnded(&TypedEventHandler::<MediaPlayer, IInspectable>::new(
        move |_, _| {
            slot.lock().unwrap().ended = true;
            Ok(())
        },
    ))?;
    let uri = Uri::CreateUri(&HSTRING::from(url))?;
    player.SetSource(&MediaSource::CreateFromUri(&uri)?)?;
    Ok(WinVideo { player, shared })
}

fn span(d: Duration) -> TimeSpan {
    TimeSpan {
        Duration: (d.as_nanos() / 100) as i64,
    }
}

fn from_span(t: TimeSpan) -> Duration {
    Duration::from_nanos(t.Duration.max(0) as u64 * 100)
}

impl Video for WinVideo {
    fn play(&self) {
        self.shared.lock().unwrap().ended = false;
        let _ = self.player.Play();
    }
    fn pause(&self) {
        let _ = self.player.Pause();
    }
    fn seek(&self, to: Duration) {
        self.shared.lock().unwrap().ended = false;
        if let Ok(session) = self.player.PlaybackSession() {
            let _ = session.SetPosition(span(to));
        }
    }
    fn set_volume(&self, volume: f32) {
        let _ = self.player.SetVolume(volume.clamp(0.0, 1.0) as f64);
    }
    fn set_muted(&self, muted: bool) {
        let _ = self.player.SetIsMuted(muted);
    }
    fn state(&self) -> State {
        let shared = self.shared.lock().unwrap();
        if shared.failed {
            return State::Failed;
        }
        if shared.ended {
            return State::Ended;
        }
        let state = self
            .player
            .PlaybackSession()
            .and_then(|s| s.PlaybackState())
            .unwrap_or(MediaPlaybackState::None);
        match state {
            MediaPlaybackState::Playing => State::Playing,
            MediaPlaybackState::Paused => State::Paused,
            _ => State::Loading,
        }
    }
    fn position(&self) -> Duration {
        self.player
            .PlaybackSession()
            .and_then(|s| s.Position())
            .map(from_span)
            .unwrap_or_default()
    }
    fn duration(&self) -> Option<Duration> {
        self.player
            .PlaybackSession()
            .and_then(|s| s.NaturalDuration())
            .ok()
            .map(from_span)
            .filter(|d| !d.is_zero())
    }
    fn frame(&self, after: u64) -> Option<Frame> {
        let shared = self.shared.lock().unwrap();
        shared.frame.as_ref().filter(|f| f.serial > after).cloned()
    }
}

impl Drop for WinVideo {
    fn drop(&mut self) {
        let _ = self.player.Close();
    }
}
