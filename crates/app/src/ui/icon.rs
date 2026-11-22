//! Icons from gpui-kit's set, sized and coloured for this app.

pub use gpui_kit::assets::IconName as LucideIcon;
pub use gpui_kit::component::IconName;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;

gpui_kit::assets::icon_assets!(
    WorkspaceIcons,
    [
        Bookmark,
        Cloud,
        CloudOff,
        Image,
        Link,
        Monitor,
        Paperclip,
        Pencil,
        Send,
        ShieldCheck,
        Slash,
        Upload,
        Users
    ]
);

pub(crate) struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match WorkspaceIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(WorkspaceIcons.list(path)?);
        Ok(paths)
    }
}

/// A small icon in `color`.
pub fn icon(name: impl Into<Icon>, color: Hsla) -> Icon {
    Icon::new(name).small().text_color(color)
}

/// An icon button: an icon with an ID, the button role and `label` as its
/// accessible name and tooltip.
pub fn button(
    id: impl Into<ElementId>,
    name: impl Into<Icon>,
    label: impl Into<SharedString>,
    theme: &crate::theme::Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    use super::focus::Reachable;
    use gpui_kit::TestSupportExt;
    let label: SharedString = label.into();
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(label.clone())
        .tooltip(super::tooltip::text(label))
        .p_1()
        .rounded_md()
        .cursor_pointer()
        .text_color(theme.muted)
        .hover({
            let (hover, text) = (theme.hover, theme.text);
            move |d| d.bg(hover).text_color(text)
        })
        .child(icon(name, theme.muted))
        .on_click(on_click)
}
