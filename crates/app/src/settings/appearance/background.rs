//! The background picture of a personal theme, in the theme editor: choose an
//! image file, replace or remove it, and say where it sits and how strongly it
//! is dimmed, blurred and covered by the panels. The web's theme studio keeps
//! the same draft (`ThemeStudio.vue`, Background): the picture is uploaded as
//! soon as it is chosen and is stored with the theme only when the theme is
//! saved.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::ApiError;
use ircord_client::api::uploads::{FileData, Progress, STALL};
use ircord_client::contract::{
    ThemeBackground, ThemeBackgroundFit as Fit, ThemeBackgroundPlacement as Placement,
    ThemeBackgroundPosition as Position,
};
use ircord_client::store::appearance::{background_floors, default_background, instance_file_path};

use super::{Editor, option};
use crate::settings::profile::error_text;
use crate::theme::{Theme, appearance, backdrop};
use crate::ui::focus::Reachable;

/// The dim, blur and panel sliders, in that order.
const RANGES: [(f32, f32, f32); 3] = [(0., 90., 5.), (0., 20., 1.), (40., 100., 5.)];

/// The background of the theme being edited, until it is saved or dropped.
pub(super) struct Draft {
    /// What will be saved with the theme: `None` is no background.
    pub value: Option<ThemeBackground>,
    /// A small copy of the picture, and the path it was made from.
    thumb: Option<(String, Arc<RenderImage>)>,
    uploading: bool,
    error: Option<SharedString>,
    sliders: [Entity<SliderState>; 3],
    /// The value each slider was last set to from the draft; a slider the
    /// draft has moved from under is set again.
    seen: [i64; 3],
    events: Vec<Subscription>,
}

impl Draft {
    pub(super) fn new(cx: &mut App) -> Self {
        let slider = |(min, max, step): (f32, f32, f32), cx: &mut App| {
            cx.new(|_| SliderState::new().min(min).max(max).step(step))
        };
        Self {
            value: None,
            thumb: None,
            uploading: false,
            error: None,
            sliders: RANGES.map(|range| slider(range, cx)),
            seen: [-1; 3],
            events: Vec::new(),
        }
    }

    /// Start from the background `theme` has, or none.
    pub(super) fn reset(&mut self, value: Option<ThemeBackground>) {
        self.value = value;
        self.thumb = None;
        self.uploading = false;
        self.error = None;
        self.seen = [-1; 3];
    }

    pub(super) fn is_uploading(&self) -> bool {
        self.uploading
    }

    /// Follow a slider's release into the draft.
    fn released(&mut self, which: usize, value: i64, cx: &mut App) {
        let tokens = appearance::compiled(cx).tokens;
        let background = self.value.get_or_insert_with(default_background);
        let (dim, panel) = background_floors(&tokens, background);
        match which {
            0 => background.dim = Some(value.max(dim)),
            1 => background.blur = Some(value),
            _ => background.panel_opacity = Some(value.max(panel)),
        }
        self.seen[which] = -1;
        cx.refresh_windows();
    }
}

/// Listen to the sliders; the editor exists by now.
pub(super) fn listen(editor: &Entity<Editor>, cx: &mut App) {
    let sliders = editor.read(cx).background.sliders.clone();
    let events = editor.update(cx, |_, cx| {
        sliders
            .iter()
            .enumerate()
            .map(|(which, slider)| {
                cx.subscribe(slider, move |e: &mut Editor, _, event: &SliderEvent, cx| {
                    if let SliderEvent::Release(at) = event {
                        e.background.released(which, at.end().round() as i64, cx);
                    }
                })
            })
            .collect()
    });
    editor.update(cx, |e, _| e.background.events = events);
}

/// Read the picture `path` names from the account, to show it.
pub(super) fn load_thumb(editor: &Entity<Editor>, path: String, cx: &mut App) {
    let Some(client) = editor.read(cx).session.read(cx).client().cloned() else {
        return;
    };
    let wanted = path.clone();
    let task = client.request(move |api| async move { api.theme_picture(&wanted).await });
    let editor = editor.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(bytes)) = task.await else {
            return;
        };
        let Some((image, _)) = cx
            .background_spawn(async move { backdrop::preview(&bytes) })
            .await
        else {
            return;
        };
        cx.update(|cx| {
            editor.update(cx, |e, _| {
                let still = e
                    .background
                    .value
                    .as_ref()
                    .and_then(|b| b.url.as_deref())
                    .is_some_and(|url| url == path);
                if still && e.background.thumb.is_none() {
                    e.background.thumb = Some((path, image));
                }
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

fn say(editor: &Entity<Editor>, text: Option<SharedString>, cx: &mut App) {
    editor.update(cx, |e, _| e.background.error = text);
    cx.refresh_windows();
}

/// Ask for an image, upload it to the account and make it the draft's
/// picture, keeping the placement settings already chosen.
fn choose(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) {
    let session = editor.read(cx).session.clone();
    if editor.read(cx).background.uploading {
        return;
    }
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let limits = client.read().capabilities.upload.clone();
    let network = {
        let w = client.read();
        session
            .read(cx)
            .network
            .clone()
            .filter(|id| w.network(id).is_some())
            .or_else(|| w.networks.first().map(|n| n.id.clone()))
    };
    let Some(network) = network else {
        return;
    };
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(t!("appearance-bg-choose").into()),
    });
    let editor = editor.clone();
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let mut refuse = |text: &'static str| {
                cx.update(|_, cx| say(&editor, Some(text.into()), cx)).ok();
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "image".into());
            let Some(bytes) = read(&path) else {
                return refuse(t!("appearance-bg-not-image"));
            };
            let file = FileData::named(name, bytes);
            if !file.is_image() {
                return refuse(t!("appearance-bg-not-image"));
            }
            if limits
                .as_ref()
                .is_some_and(|l| l.max_bytes > 0 && file.size() > l.max_bytes as u64)
            {
                return refuse(t!("appearance-bg-too-big"));
            }
            cx.update(|_, cx| {
                editor.update(cx, |e, _| {
                    e.background.uploading = true;
                    e.background.error = None;
                });
                cx.refresh_windows();
            })
            .ok();
            let measured = {
                let bytes = file.bytes.clone();
                cx.background_spawn(async move { backdrop::preview(&bytes) })
            };
            let task = client.request(move |api| async move {
                let upload = api
                    .upload_file(&network, &file, &Progress::default(), STALL)
                    .await
                    .map_err(|e| ApiError::Http {
                        status: 0,
                        detail: e.to_string(),
                    })?;
                instance_file_path(&upload.url, api.server().url()).ok_or(ApiError::Http {
                    status: 0,
                    detail: t!("appearance-bg-not-instance").into(),
                })
            });
            let result = task.await;
            let measured = measured.await;
            cx.update(|_, cx| {
                editor.update(cx, |e, _| {
                    let draft = &mut e.background;
                    draft.uploading = false;
                    match result {
                        Ok(Ok(url)) => {
                            let mut next = draft.value.take().unwrap_or_else(default_background);
                            next.url = Some(url.clone());
                            next.tones = measured
                                .as_ref()
                                .and_then(|(_, tones)| tones.clone())
                                .map(Vec::from);
                            draft.value = Some(next);
                            draft.thumb = measured.map(|(image, _)| (url, image));
                            draft.seen = [-1; 3];
                        }
                        Ok(Err(e)) => draft.error = Some(error_text(&e).into()),
                        Err(e) => draft.error = Some(e.to_string().into()),
                    }
                });
                cx.refresh_windows();
            })
            .ok();
        })
        .detach();
}

fn read(path: &PathBuf) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// A row of options for one setting of the draft.
fn choices<T: Copy + PartialEq + 'static>(
    prefix: &'static str,
    label: &'static str,
    options: &[(T, &'static str, &'static str)],
    selected: T,
    set: fn(&mut ThemeBackground, T),
    editor: &Entity<Editor>,
    theme: &Theme,
) -> Div {
    let mut list = div()
        .id(ElementId::Name(format!("{prefix}-choices").into()))
        .test_support()
        .role(Role::RadioGroup)
        .aria_label(label)
        .flex()
        .flex_wrap()
        .gap_1();
    for (value, key, text) in options {
        let (value, editor) = (*value, editor.clone());
        list = list.child(
            option(format!("{prefix}-{key}"), *text, value == selected, theme)
                .test_support()
                .reachable(theme)
                .on_click(move |_, _, cx| {
                    editor.update(cx, |e, _| {
                        let draft = &mut e.background;
                        set(draft.value.get_or_insert_with(default_background), value);
                    });
                    cx.refresh_windows();
                }),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(theme.muted).child(label))
        .child(list)
}

/// A slider with its value and a line of help.
fn slider(
    id: &'static str,
    label: &'static str,
    state: &Entity<SliderState>,
    help: Option<String>,
    theme: &Theme,
    cx: &App,
) -> Div {
    let value = state.read(cx).value().end().round() as i64;
    let value = SharedString::from(match id {
        "appearance-bg-blur" => t!("appearance-bg-pixels", pixels = value),
        _ => t!("appearance-bg-percent", percent = value),
    });
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(theme.muted).child(label))
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .id(ElementId::Name(format!("{id}-track").into()))
                        .test_support()
                        .w(px(220.))
                        .child(
                            Slider::new(state)
                                .accessibility_label(label)
                                .bg(theme.accent)
                                .text_color(theme.main),
                        ),
                )
                .child(
                    div()
                        .id(ElementId::Name(format!("{id}-value").into()))
                        .test_support()
                        .role(Role::Status)
                        .aria_label(value.clone())
                        .w(px(56.))
                        .text_center()
                        .text_color(theme.text)
                        .child(value),
                ),
        )
        .children(help.map(|help| div().text_sm().text_color(theme.muted).child(help)))
}

/// The Background part of the theme editor.
pub(super) fn section(
    editor: &Entity<Editor>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let allowed = editor.read(cx).session.read(cx).client().is_some_and(|c| {
        c.read()
            .capabilities
            .upload
            .as_ref()
            .is_some_and(|u| u.types.iter().any(|t| t.starts_with("image/")))
    });
    let heading = div()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .child(t!("appearance-bg"));
    let mut body = div().flex().flex_col().gap_2().child(heading);
    if !allowed {
        return body.child(
            div()
                .id("appearance-bg-unavailable")
                .test_support()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-bg-no-uploads")),
        );
    }

    let tokens = appearance::compiled(cx).tokens;
    let draft = &editor.read(cx).background;
    let background = draft.value.clone();
    let (dim_floor, panel_floor) = background
        .as_ref()
        .map(|b| background_floors(&tokens, b))
        .unwrap_or_else(|| background_floors(&tokens, &default_background()));
    let wanted = |value: Option<i64>, default: i64, floor: i64| value.unwrap_or(default).max(floor);
    let level = [
        wanted(background.as_ref().and_then(|b| b.dim), 55, dim_floor),
        wanted(background.as_ref().and_then(|b| b.blur), 0, 0),
        wanted(
            background.as_ref().and_then(|b| b.panel_opacity),
            85,
            panel_floor,
        ),
    ];
    for (which, slider) in draft.sliders.clone().iter().enumerate() {
        if editor.read(cx).background.seen[which] != level[which] {
            slider.update(cx, |s, cx| s.set_value(level[which] as f32, window, cx));
            editor.update(cx, |e, _| e.background.seen[which] = level[which]);
        }
    }

    let draft = &editor.read(cx).background;
    let (uploading, error) = (draft.uploading, draft.error.clone());
    let url = background.as_ref().and_then(|b| b.url.clone());
    let thumb = draft
        .thumb
        .as_ref()
        .filter(|(path, _)| Some(path) == url.as_ref())
        .map(|(_, image)| image.clone());

    if background.is_some() && url.is_none() {
        body = body.child(
            div()
                .id("appearance-bg-needs-image")
                .test_support()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-bg-needs-image")),
        );
    }
    if url.is_some() {
        body = body.child(
            div()
                .id("appearance-bg-thumb")
                .test_support()
                .role(Role::Image)
                .aria_label(t!("appearance-bg-current"))
                .h(px(120.))
                .rounded(theme.radius)
                .overflow_hidden()
                .when_some(thumb, |d, image| {
                    d.child(img(image).h_full().object_fit(ObjectFit::Contain))
                }),
        );
    }
    let choose_editor = editor.clone();
    let remove_editor = editor.clone();
    body = body.child(
        div()
            .flex()
            .gap_2()
            .child(
                super::button(
                    "appearance-bg-choose",
                    if uploading {
                        t!("appearance-bg-uploading")
                    } else if url.is_some() {
                        t!("appearance-bg-replace")
                    } else {
                        t!("appearance-bg-choose")
                    },
                    theme,
                )
                .test_support()
                .reachable(theme)
                .when(uploading, |d| d.opacity(0.6))
                .on_click(move |_, window, cx| choose(&choose_editor, window, cx)),
            )
            .when(background.is_some(), |row| {
                row.child(
                    super::button("appearance-bg-remove", t!("appearance-bg-remove"), theme)
                        .test_support()
                        .reachable(theme)
                        .text_color(theme.danger)
                        .on_click(move |_, _, cx| {
                            remove_editor.update(cx, |e, _| e.background.reset(None));
                            cx.refresh_windows();
                        }),
                )
            }),
    );
    if let Some(error) = error {
        body = body.child(
            div()
                .id("appearance-bg-error")
                .test_support()
                .role(Role::Alert)
                .aria_label(error.clone())
                .text_sm()
                .text_color(theme.danger)
                .child(error),
        );
    }
    body = body.child(
        div()
            .text_sm()
            .text_color(theme.muted)
            .child(t!("appearance-bg-help")),
    );
    let Some(background) = background else {
        return body;
    };

    let placement = background.placement.unwrap_or(Placement::Conversation);
    let fit = background.fit.unwrap_or(Fit::Cover);
    let position = background.position.unwrap_or(Position::Center);
    let sliders = editor.read(cx).background.sliders.clone();
    body = body
        .child(choices(
            "appearance-bg-placement",
            t!("appearance-bg-placement"),
            &[
                (
                    Placement::Conversation,
                    "conversation",
                    t!("appearance-bg-placement-conversation"),
                ),
                (Placement::App, "app", t!("appearance-bg-placement-app")),
            ],
            placement,
            |b, v| b.placement = Some(v),
            editor,
            theme,
        ))
        .child(choices(
            "appearance-bg-fit",
            t!("appearance-bg-fit"),
            &[
                (Fit::Cover, "cover", t!("appearance-bg-fit-cover")),
                (Fit::Contain, "contain", t!("appearance-bg-fit-contain")),
                (Fit::Tile, "tile", t!("appearance-bg-fit-tile")),
            ],
            fit,
            |b, v| b.fit = Some(v),
            editor,
            theme,
        ))
        .child(choices(
            "appearance-bg-position",
            t!("appearance-bg-position"),
            &[
                (
                    Position::Center,
                    "center",
                    t!("appearance-bg-position-center"),
                ),
                (Position::Top, "top", t!("appearance-bg-position-top")),
                (
                    Position::Bottom,
                    "bottom",
                    t!("appearance-bg-position-bottom"),
                ),
                (Position::Left, "left", t!("appearance-bg-position-left")),
                (Position::Right, "right", t!("appearance-bg-position-right")),
            ],
            position,
            |b, v| b.position = Some(v),
            editor,
            theme,
        ))
        .child(slider(
            "appearance-bg-dim",
            t!("appearance-bg-dim"),
            &sliders[0],
            Some(t!("appearance-bg-dim-help", floor = dim_floor)),
            theme,
            cx,
        ))
        .child(slider(
            "appearance-bg-blur",
            t!("appearance-bg-blur"),
            &sliders[1],
            None,
            theme,
            cx,
        ));
    if placement == Placement::App {
        body = body.child(slider(
            "appearance-bg-panel",
            t!("appearance-bg-panels"),
            &sliders[2],
            Some(t!("appearance-bg-panels-help", floor = panel_floor)),
            theme,
            cx,
        ));
    }
    body
}
