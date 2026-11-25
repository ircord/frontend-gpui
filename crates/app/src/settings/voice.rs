//! Settings → Voice: the microphone and speaker voice channels use, and the
//! microphone's processing (echo cancellation, noise suppression, automatic
//! gain), as the web client's `VoiceVideoSettings.vue`. They are kept on this
//! device (`ircord_client::voice::AudioSettings`) and apply at once, to a
//! call under way too.
//!
//! The devices are read from the media layer when the section is drawn,
//! off the UI thread (opening the system's sound devices can take a
//! moment), and read again when it is next drawn after five seconds.

use std::time::{Duration, Instant};

use gpui_kit::TestSupportExt;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::voice::{AudioSettings, Device, Devices};

use super::row;
use crate::features::channel_settings::choices;
use crate::features::voice;
use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::theme::{self, Theme};

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new("voice", t!("voice-settings-title"), render)
                .group(SettingsGroup::App)
                // After Notifications and before This device, as the web's
                // "Voice & video".
                .order(33)
                .keywords(t!("voice-settings-keywords").split(", "))
                .row(
                    SettingsRow::new("voice-microphone", t!("voice-microphone"))
                        .description(t!("voice-microphone-help"))
                        .keywords(t!("voice-microphone-keywords")),
                )
                .row(
                    SettingsRow::new("voice-speaker", t!("voice-speaker"))
                        .description(t!("voice-speaker-help"))
                        .keywords(t!("voice-speaker-keywords")),
                )
                .row(
                    SettingsRow::new("voice-echo-cancellation", t!("voice-echo-cancellation"))
                        .description(t!("voice-echo-cancellation-help"))
                        .keywords(t!("voice-echo-cancellation-keywords")),
                )
                .row(
                    SettingsRow::new("voice-noise-suppression", t!("voice-noise-suppression"))
                        .description(t!("voice-noise-suppression-help"))
                        .keywords(t!("voice-noise-suppression-keywords")),
                )
                .row(
                    SettingsRow::new("voice-auto-gain", t!("voice-auto-gain"))
                        .description(t!("voice-auto-gain-help"))
                        .keywords(t!("voice-auto-gain-keywords")),
                ),
        );
    });
}

/// The devices last read and when, and whether a read is under way.
#[derive(Default)]
struct DeviceList {
    devices: Option<Devices>,
    read_at: Option<Instant>,
    reading: bool,
}
impl Global for DeviceList {}

/// How long a read of the devices holds: a headset plugged in meanwhile
/// appears the next time the section is drawn after that.
const DEVICES_FRESH: Duration = Duration::from_secs(5);

fn read_devices(ctx: &SettingsCtx, cx: &mut App) {
    let list = cx.default_global::<DeviceList>();
    if list.reading || list.read_at.is_some_and(|at| at.elapsed() < DEVICES_FRESH) {
        return;
    }
    list.reading = true;
    let connector = voice::connector(&ctx.session, cx);
    let task = cx.background_spawn(async move { connector.devices() });
    cx.spawn(async move |cx| {
        let devices = task.await;
        cx.update(|cx| {
            let list = cx.global_mut::<DeviceList>();
            list.reading = false;
            list.read_at = Some(Instant::now());
            list.devices = Some(devices);
            cx.refresh_windows();
        });
    })
    .detach();
}

fn render(ctx: &SettingsCtx, _window: &mut Window, cx: &mut App) -> AnyElement {
    read_devices(ctx, cx);
    let theme = theme::get(cx).clone();
    let model = voice::model(&ctx.session, cx);
    let settings = model.read(cx).settings.clone();
    let devices = cx.global::<DeviceList>().devices.clone();
    let update = move |change: fn(&mut AudioSettings, Option<String>, bool)| {
        let model = model.clone();
        move |value: Option<String>, on: bool, cx: &mut App| {
            model.update(cx, |m, cx| {
                let mut settings = m.settings.clone();
                change(&mut settings, value, on);
                m.set_settings(settings, cx);
            });
            super::saved(cx);
            cx.refresh_windows();
        }
    };
    let set_microphone = update(|s, id, _| s.microphone = id);
    let set_speaker = update(|s, id, _| s.speaker = id);
    let set_echo = update(|s, _, on| s.echo_cancellation = on);
    let set_noise = update(|s, _, on| s.noise_suppression = on);
    let set_gain = update(|s, _, on| s.auto_gain = on);
    div()
        .id("voice-settings-page")
        .test_support()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("voice-applies-now")),
        )
        .child(row(
            ctx,
            "voice-microphone",
            t!("voice-microphone"),
            Some(t!("voice-microphone-help").into()),
            device_choice(
                "voice-microphone-choice",
                t!("voice-microphone"),
                devices.as_ref().map(|d| d.microphones.as_slice()),
                settings.microphone.clone(),
                &theme,
                set_microphone,
            ),
            &theme,
        ))
        .child(row(
            ctx,
            "voice-speaker",
            t!("voice-speaker"),
            Some(t!("voice-speaker-help").into()),
            device_choice(
                "voice-speaker-choice",
                t!("voice-speaker"),
                devices.as_ref().map(|d| d.speakers.as_slice()),
                settings.speaker.clone(),
                &theme,
                set_speaker,
            ),
            &theme,
        ))
        .child(switch_row(
            ctx,
            "voice-echo-cancellation",
            t!("voice-echo-cancellation"),
            t!("voice-echo-cancellation-help"),
            settings.echo_cancellation,
            &theme,
            set_echo,
        ))
        .child(switch_row(
            ctx,
            "voice-noise-suppression",
            t!("voice-noise-suppression"),
            t!("voice-noise-suppression-help"),
            settings.noise_suppression,
            &theme,
            set_noise,
        ))
        .child(switch_row(
            ctx,
            "voice-auto-gain",
            t!("voice-auto-gain"),
            t!("voice-auto-gain-help"),
            settings.auto_gain,
            &theme,
            set_gain,
        ))
        .into_any_element()
}

/// The system default, then each device; a chosen device that is not
/// connected stays listed, so it can be seen and changed.
fn device_choice(
    id: &'static str,
    label: &'static str,
    devices: Option<&[Device]>,
    chosen: Option<String>,
    theme: &Theme,
    pick: impl Fn(Option<String>, bool, &mut App) + 'static,
) -> AnyElement {
    let Some(devices) = devices else {
        return note(t!("voice-devices-loading"), theme);
    };
    let mut ids: Vec<Option<String>> = vec![None];
    let mut options = vec![(0usize, SharedString::from(t!("voice-system-default")))];
    for device in devices {
        ids.push(Some(device.id.clone()));
        options.push((options.len(), SharedString::from(device.name.clone())));
    }
    if let Some(missing) = chosen
        .as_ref()
        .filter(|c| !devices.iter().any(|d| &d.id == *c))
    {
        ids.push(Some(missing.clone()));
        options.push((
            options.len(),
            t!("voice-missing-device", device = missing.as_str()).into(),
        ));
    }
    let picked = ids.iter().position(|i| *i == chosen).unwrap_or(0);
    let column = div()
        .max_w(px(320.))
        .flex()
        .flex_col()
        .items_end()
        .gap_1()
        .child(choices(
            id,
            label,
            options,
            picked,
            true,
            move |i, _, cx| pick(ids.get(i).cloned().flatten(), true, cx),
        ));
    if devices.is_empty() {
        column
            .child(note(t!("voice-no-devices"), theme))
            .into_any_element()
    } else {
        column.into_any_element()
    }
}

fn note(text: &'static str, theme: &Theme) -> AnyElement {
    div()
        .text_sm()
        .text_color(theme.muted)
        .child(text)
        .into_any_element()
}

fn switch_row(
    ctx: &SettingsCtx,
    id: &'static str,
    label: &'static str,
    help: &'static str,
    on: bool,
    theme: &Theme,
    set: impl Fn(Option<String>, bool, &mut App) + 'static,
) -> impl IntoElement {
    row(
        ctx,
        id,
        label,
        Some(help.into()),
        Switch::new(SharedString::from(format!("{id}-switch")))
            .checked(on)
            .accessibility_label(label)
            .on_click(move |on, _, cx| set(None, *on, cx)),
        theme,
    )
}
