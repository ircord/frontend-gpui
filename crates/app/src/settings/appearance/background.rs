//! Edit the image in a personal theme. Changes stay in the theme draft until Save.

use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::{
    ApiError,
    uploads::{self, FileData, Progress, UploadUse},
};
use ircord_client::contract::{
    Theme as UserTheme, ThemeBackground, ThemeBackgroundFit as Fit,
    ThemeBackgroundPlacement as Placement, ThemeBackgroundPosition as Position, UploadCapability,
};
use ircord_client::store::appearance::background::MAX_IMAGE_BYTES;
use ircord_client::store::appearance::{self as look, Backdrop, BackdropPlacement, Mode};

use super::{Editor, option, say};
use crate::theme::{Theme, appearance, background as wallpaper};
use crate::ui::focus::Reachable;

pub(super) fn render(
    editor: &Entity<Editor>,
    draft: &UserTheme,
    mode: Mode,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let state = editor.read(cx);
    let client = state.session.read(cx).client().cloned();
    let preview = state.background_preview.clone();
    let busy = state.busy;
    let uploading = state.uploading;
    let can_upload = client.as_ref().is_some_and(|client| {
        client
            .read()
            .capabilities
            .upload
            .as_ref()
            .is_some_and(|limits| limits.types.iter().any(|kind| kind.starts_with("image/")))
    });
    let mut look = appearance::look(cx);
    look.theme.clone_from(&draft.id);
    let compiled = look::compile_with_css(
        &look,
        mode,
        std::slice::from_ref(draft),
        false,
        appearance::css_allowed(cx),
    );
    let palette = Theme::from_compiled(&compiled);
    let image = draft.background.as_ref();
    let background = look::theme_backdrop(draft, &compiled.tokens);
    preview.update(cx, |view, cx| {
        view.select(client.as_ref().zip(background.as_ref()), window, cx)
    });

    let choose = editor.clone();
    let remove = editor.clone();
    let mut form = div().flex().flex_col().gap_3().child(
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .when(can_upload, |row| {
                row.child(
                    Button::new("theme-background-choose")
                        .label(if uploading {
                            t!("appearance-bg-uploading")
                        } else if image.is_some_and(|image| image.url.is_some()) {
                            t!("appearance-background-replace")
                        } else {
                            t!("appearance-background-choose")
                        })
                        .outline()
                        .disabled(busy)
                        .on_click(move |_, _, cx| upload(&choose, cx)),
                )
            })
            .when(image.is_some(), |row| {
                row.child(
                    Button::new("theme-background-remove")
                        .label(t!("appearance-background-remove"))
                        .ghost()
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            remove.update(cx, |editor, _| {
                                if let Some((draft, _)) = &mut editor.editing {
                                    draft.background = None;
                                }
                            });
                            super::refresh_studio(&remove, cx);
                        }),
                )
            }),
    );
    if !can_upload || image.is_some_and(|image| image.url.is_none()) {
        form = form.child(
            div()
                .id("theme-background-unavailable")
                .test_support()
                .text_sm()
                .text_color(theme.muted)
                .child(if can_upload {
                    t!("appearance-background-needs-image")
                } else {
                    t!("appearance-background-unavailable")
                }),
        );
    }
    let Some(image) = image else {
        return form;
    };
    if let Some(bg) = background.as_ref() {
        form = form.child(render_preview(preview, bg, &palette, cx));
    }
    let placement = image.placement.unwrap_or(Placement::Conversation);
    let (dim_floor, panel_floor) = look::background_floors(&compiled.tokens, image);
    form.child(options(
        editor,
        "placement",
        t!("appearance-background-placement"),
        &[
            (
                Placement::Conversation,
                "conversation",
                t!("appearance-background-conversation"),
            ),
            (Placement::App, "app", t!("appearance-background-app")),
        ],
        placement,
        |bg, value| bg.placement = Some(value),
        theme,
    ))
    .child(options(
        editor,
        "fit",
        t!("appearance-background-fit"),
        &[
            (Fit::Cover, "cover", t!("appearance-background-cover")),
            (Fit::Contain, "contain", t!("appearance-background-contain")),
            (Fit::Tile, "tile", t!("appearance-background-tile")),
        ],
        image.fit.unwrap_or(Fit::Cover),
        |bg, value| bg.fit = Some(value),
        theme,
    ))
    .child(options(
        editor,
        "position",
        t!("appearance-background-position"),
        &[
            (
                Position::Center,
                "center",
                t!("appearance-background-center"),
            ),
            (Position::Top, "top", t!("appearance-background-top")),
            (
                Position::Bottom,
                "bottom",
                t!("appearance-background-bottom"),
            ),
            (Position::Left, "left", t!("appearance-background-left")),
            (Position::Right, "right", t!("appearance-background-right")),
        ],
        image.position.unwrap_or(Position::Center),
        |bg, value| bg.position = Some(value),
        theme,
    ))
    .child(slider(
        editor,
        Field::Dim,
        image.dim.unwrap_or(55),
        (dim_floor, 90, 5),
        window,
        cx,
    ))
    .child(slider(
        editor,
        Field::Blur,
        image.blur.unwrap_or(0),
        (0, 20, 1),
        window,
        cx,
    ))
    .when(placement == Placement::App, |form| {
        form.child(slider(
            editor,
            Field::Panels,
            image.panel_opacity.unwrap_or(85),
            (panel_floor, 100, 5),
            window,
            cx,
        ))
    })
}

fn render_preview(
    picture: Entity<wallpaper::Wallpaper>,
    bg: &Backdrop,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement + use<> {
    let app = bg.placement == BackdropPlacement::App;
    let dim = bg.dim;
    let sample = div()
        .relative()
        .min_w_0()
        .flex_1()
        .p_3()
        .bg(theme.main.opacity(if app { dim } else { 1. }))
        .when(!app, |d| {
            d.child(wallpaper::layer(
                picture.clone(),
                bg,
                1. - dim,
                Corners::default(),
            ))
        })
        .child(crate::settings::preview_samples(
            theme,
            crate::message::layout::settings(cx),
            "theme-background-sample",
        ));
    div()
        .id("theme-background-preview")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("appearance-background-preview"))
        .relative()
        .flex()
        .min_h(px(112.))
        .w_full()
        .overflow_hidden()
        .rounded(theme.radius)
        .bg(theme.sidebar)
        .border_1()
        .border_color(theme.border)
        .when(app, |d| {
            d.child(wallpaper::layer(
                picture,
                bg,
                1. - bg.panel,
                Corners::default(),
            ))
        })
        .child(
            div()
                .w(px(42.))
                .flex_none()
                .border_r_1()
                .border_color(theme.border),
        )
        .child(sample)
}

fn options<T: Copy + PartialEq + 'static>(
    editor: &Entity<Editor>,
    name: &str,
    label: &'static str,
    options: &[(T, &str, &'static str)],
    selected: T,
    set: fn(&mut ThemeBackground, T),
    theme: &Theme,
) -> Div {
    let choices = div()
        .id(ElementId::Name(format!("theme-background-{name}").into()))
        .test_support()
        .role(Role::RadioGroup)
        .aria_label(label)
        .flex()
        .flex_wrap()
        .gap_1()
        .children(options.iter().map(|&(value, key, label)| {
            let editor = editor.clone();
            option(
                format!("theme-background-{name}-{key}"),
                label,
                selected == value,
                theme,
            )
            .test_support()
            .reachable(theme)
            .on_click(move |_, _, cx| edit(&editor, cx, |bg| set(bg, value)))
        }));
    ui_field(label, choices, theme)
}

fn ui_field(label: &'static str, control: impl IntoElement, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(theme.text).child(label))
        .child(control)
}

#[derive(Clone, Copy)]
enum Field {
    Dim,
    Blur,
    Panels,
}

impl Field {
    fn label(self) -> (&'static str, &'static str) {
        match self {
            Self::Dim => ("dim", t!("appearance-background-dim")),
            Self::Blur => ("blur", t!("appearance-background-blur")),
            Self::Panels => ("panels", t!("appearance-background-panels")),
        }
    }

    fn set(self, bg: &mut ThemeBackground, value: i64) {
        match self {
            Self::Dim => bg.dim = Some(value),
            Self::Blur => bg.blur = Some(value),
            Self::Panels => bg.panel_opacity = Some(value),
        }
    }
}

fn slider(
    editor: &Entity<Editor>,
    field: Field,
    value: i64,
    (min, max, step): (i64, i64, i64),
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let (id, label) = field.label();
    let generation = editor.read(cx).edit_generation;
    let editor = editor.clone();
    let state = window.use_keyed_state(
        ElementId::Name(format!("theme-bg-slider-{id}-{generation}-{min}").into()),
        cx,
        |_, cx| {
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(min as f32)
                    .max(max as f32)
                    .step(step as f32)
                    .default_value(value.clamp(min, max) as f32)
            });
            let subscription = cx.subscribe(&slider, move |_, _, event: &SliderEvent, cx| {
                if let SliderEvent::Change(at) = event
                    && editor.read(cx).edit_generation == generation
                {
                    edit(&editor, cx, |bg| field.set(bg, at.end().round() as i64));
                }
            });
            (slider, subscription)
        },
    );
    let slider = &state.read(cx).0;
    let shown = slider.read(cx).value().end().round() as i64;
    let value = if matches!(field, Field::Blur) {
        t!("appearance-background-px", value = shown)
    } else {
        t!("appearance-background-percent", value = shown)
    };
    let theme = crate::theme::get(cx);
    ui_field(
        label,
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id(ElementId::Name(
                        format!("theme-background-{id}-track").into(),
                    ))
                    .test_support()
                    .flex_1()
                    .max_w(px(220.))
                    .min_w_0()
                    .child(
                        Slider::new(slider)
                            .accessibility_label(label)
                            .bg(theme.accent)
                            .text_color(theme.main),
                    ),
            )
            .child(div().text_sm().text_color(theme.muted).child(value)),
        theme,
    )
}

fn edit(editor: &Entity<Editor>, cx: &mut App, change: impl FnOnce(&mut ThemeBackground)) {
    editor.update(cx, |editor, _| {
        if let Some((draft, _)) = &mut editor.editing
            && let Some(bg) = &mut draft.background
        {
            change(bg);
        }
    });
    super::refresh_studio(editor, cx);
}

fn upload(editor: &Entity<Editor>, cx: &mut App) {
    let current = editor.read(cx);
    if current.busy || current.editing.is_none() {
        return;
    }
    let session = current.session.read(cx);
    let Some(client) = session.client().cloned() else {
        return;
    };
    let Some(limits) = client.read().capabilities.upload.clone() else {
        return;
    };
    let network = session.network.clone().unwrap_or_default();
    let origin = client.api().server().url().clone();
    let generation = current.edit_generation;
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(t!("appearance-background-choose").into()),
    });
    editor.update(cx, |editor, _| {
        editor.busy = true;
        editor.uploading = true;
        editor.notice = None;
    });
    cx.refresh_windows();
    let executor = cx.background_executor().clone();
    let editor = editor.clone();
    cx.spawn(async move |cx| {
        let path = match picked.await {
            Ok(Ok(paths)) => Ok(paths.and_then(|paths| paths.into_iter().next())),
            _ => Err(t!("appearance-background-open-failed").to_string()),
        };
        let result = match path {
            Ok(Some(path)) => match executor.spawn(async move { prepare(&path, &limits) }).await {
                Ok((file, tones)) => {
                    // The file dialog can outlive a sign-out or account switch.
                    let current = cx.update(|cx| in_progress(&editor, &client, generation, cx));
                    if !current {
                        return;
                    }
                    let uploaded = client
                        .request(move |api| async move {
                            api.upload_file(
                                &network,
                                UploadUse::Other,
                                &file,
                                &Progress::default(),
                                uploads::STALL,
                            )
                            .await
                            .map_err(|err| ApiError::Decode(err.to_string()))
                        })
                        .await;
                    match uploaded {
                        Ok(Ok(upload)) => look::instance_file_path(&upload.url, &origin)
                            .map(|path| Some((path, tones)))
                            .ok_or_else(|| t!("appearance-background-not-instance").to_string()),
                        Ok(Err(err)) => Err(t!("appearance-background-upload-failed", error = err)),
                        Err(_) => Err(t!("appearance-background-open-failed").to_string()),
                    }
                }
                Err(err) => Err(err.to_string()),
            },
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        };
        cx.update(|cx| {
            if !in_progress(&editor, &client, generation, cx) {
                return;
            }
            editor.update(cx, |editor, _| {
                editor.busy = false;
                editor.uploading = false;
            });
            match result {
                Ok(Some((path, tones))) => editor.update(cx, |editor, _| {
                    if let Some((draft, _)) = &mut editor.editing {
                        let bg = draft
                            .background
                            .get_or_insert_with(look::default_background);
                        bg.url = Some(path);
                        bg.tones = tones;
                    }
                }),
                Ok(None) => (),
                Err(err) => say(&editor, err, true, cx),
            }
            super::refresh_studio(&editor, cx);
        });
    })
    .detach();
}

fn in_progress(
    editor: &Entity<Editor>,
    client: &Rc<ircord_client::Client>,
    generation: u64,
    cx: &App,
) -> bool {
    let editor = editor.read(cx);
    editor.edit_generation == generation
        && editor
            .session
            .read(cx)
            .client()
            .is_some_and(|now| Rc::ptr_eq(now, client))
}

fn prepare(
    path: &Path,
    limits: &UploadCapability,
) -> Result<(FileData, Option<Vec<String>>), &'static str> {
    let max = if limits.max_bytes > 0 {
        (limits.max_bytes as u64).min(MAX_IMAGE_BYTES as u64)
    } else {
        MAX_IMAGE_BYTES as u64
    };
    let bad_file = t!("appearance-background-open-failed");
    let unsupported = t!("appearance-background-unsupported");
    let file = std::fs::File::open(path).map_err(|_| bad_file)?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| bad_file)?;
    if bytes.len() as u64 > max {
        return Err(t!("appearance-background-too-large"));
    }
    let content_type = image::guess_format(&bytes)
        .map_err(|_| unsupported)?
        .to_mime_type();
    if !content_type.starts_with("image/") || !uploads::accepts(&limits.types, content_type) {
        return Err(unsupported);
    }
    let picture = wallpaper::decode(&bytes).ok_or(unsupported)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(bad_file)?;
    Ok((
        FileData::new(name, content_type, bytes),
        measure_tones(&picture),
    ))
}

fn measure_tones(picture: &image::RgbaImage) -> Option<Vec<String>> {
    let thumbnail = image::imageops::resize(picture, 24, 24, image::imageops::FilterType::Triangle);
    look::measure_tones(thumbnail.as_raw()).map(Vec::from)
}
